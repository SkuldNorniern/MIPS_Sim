use crate::assembler::assemble;
use crate::component::RegisterName;
use crate::disassembler::disassemble;
use crate::executor::{Executor, Interpreter, Jit};
use crate::memory::{create_empty_memory, create_memory, EndianMode};
use neon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct State {
    channel: Channel,
    callback: Arc<Root<JsFunction>>,
    inner: Inner,
}

#[derive(Debug)]
struct Inner {
    clean_after_reset: bool,
    exec: Executor,
}

impl Default for Inner {
    fn default() -> Self {
        let interpreter = Interpreter::new(create_empty_memory(EndianMode::native()));

        Inner {
            clean_after_reset: true,
            exec: Executor::ExInterpreter(interpreter),
        }
    }
}

impl State {
    pub fn new(ch: Channel, callback: Root<JsFunction>) -> Self {
        let ret = State {
            channel: ch,
            callback: Arc::new(callback),
            inner: Default::default(),
        };
        ret.notify_all();
        ret
    }

    pub fn reset(&mut self) {
        self.inner = Default::default();
        self.notify_all();
    }

    pub fn assemble(&mut self, code: &str, endian: EndianMode) -> Result<(), String> {
        let segs = assemble(endian, code).map_err(|e| e.to_string())?;
        let mem = create_memory(endian, &segs);

        if endian == EndianMode::native() {
            self.inner.exec = Executor::ExJit(Jit::new(mem));
        } else {
            self.inner.exec = Executor::ExInterpreter(Interpreter::new(mem));
        }

        self.notify_all();
        Ok(())
    }

    pub fn edit_register(&mut self, r: RegisterName, val: u32) -> Result<(), ()> {
        self.inner.clean_after_reset = false;
        self.inner.exec.as_arch_mut().set_reg(r, val);

        self.notify_all();

        Ok(())
    }

    pub fn read_memory(&self, page_idx: u32, output: &mut [u8]) {
        let addr = page_idx * 4096;
        let mem = self.inner.exec.as_arch().mem();
        mem.read_into_slice(addr, output);
    }

    pub fn step(&mut self) -> Result<(), String> {
        self.inner.clean_after_reset = false;
        let result = self.step_silent();
        self.notify_all();
        result
    }

    pub fn step_silent(&mut self) -> Result<(), String> {
        self.inner.clean_after_reset = false;
        if self.inner.exec.as_arch().pc() < 0x00001000 {
            Ok(())
        } else {
            self.inner.exec.step().map_err(|x| format!("{:?}", x))
        }
    }

    pub fn exec_silent(&mut self) -> Result<(), String> {
        self.inner.clean_after_reset = false;
        if self.inner.exec.as_arch().pc() < 0x00001000 {
            Ok(())
        } else {
            self.inner.exec.exec().map_err(|x| format!("{:?}", x))
        }
    }

    pub fn run(&self, allow_jit: bool) {
        super::looper::start(allow_jit);
    }

    pub fn stop(&self) {
        super::looper::stop();
        self.notify_all()
    }

    pub fn notify_all(&self) {
        let callback = self.callback.clone();

        let clean_after_reset = self.inner.clean_after_reset;
        let regs = self.inner.capture_regs();
        let pc = self.inner.capture_pc();
        let disasm_mapping = self.inner.capture_disasm();
        let running = self.inner.capture_running();
        let can_use_jit = self.inner.capture_can_use_jit();

        self.channel.send(move |mut cx| {
            let regs = js_array_numbers(&mut cx, regs.iter())?;
            let pc = cx.number(pc);

            let disasm = cx.empty_object();
            for (k, v) in disasm_mapping.iter() {
                let number = cx.number(v.0);
                let value = cx.string(&v.1);
                let tuple = cx.empty_array();
                tuple.set(&mut cx, 0, number)?;
                tuple.set(&mut cx, 1, value)?;
                disasm.set(&mut cx, *k, tuple)?;
            }
            let mut disasm_list = disasm_mapping.keys().copied().collect::<Vec<u32>>();
            disasm_list.sort();
            let disasm_list = js_array_numbers(&mut cx, disasm_list.iter())?;
            let running = cx.boolean(running);
            let clean_after_reset = cx.boolean(clean_after_reset);
            let can_use_jit = cx.boolean(can_use_jit);

            let obj = cx.empty_object();
            obj.set(&mut cx, "regs", regs)?;
            obj.set(&mut cx, "pc", pc)?;
            obj.set(&mut cx, "disasm", disasm)?;
            obj.set(&mut cx, "disasmList", disasm_list)?;
            obj.set(&mut cx, "running", running)?;
            obj.set(&mut cx, "cleanAfterReset", clean_after_reset)?;
            obj.set(&mut cx, "canUseJit", can_use_jit)?;

            callback
                .to_inner(&mut cx)
                .call_with(&cx)
                .arg(obj)
                .exec(&mut cx)
        });
    }
}

impl Inner {
    fn capture_regs(&self) -> [u32; 32] {
        let mut ret = [0; 32];
        self.exec.as_arch().read_all_reg(&mut ret);
        ret
    }

    fn capture_pc(&self) -> u32 {
        self.exec.as_arch().pc()
    }

    fn capture_disasm(&self) -> HashMap<u32, (u32, String)> {
        let pc = self.exec.as_arch().pc();
        let mem = self.exec.as_arch().mem();
        let mut mapping = HashMap::new();

        /* walk back */
        {
            let mut addr = pc & (!0xfff);
            let mut nop_cnt: u32 = 0;
            while addr > 4096 && nop_cnt < 16 {
                let x = mem.read_u32(addr);

                if x == 0x00000000 || x == 0x00000020 {
                    nop_cnt += 1;
                } else {
                    nop_cnt = 0;
                }

                mapping.insert(addr, (x, disassemble(x)));
                addr -= 4;
            }
        }

        /* walk forward */
        {
            let mut addr = pc & (!0xfff);
            let mut nop_cnt: u32 = 0;
            while addr < 0x1000_0000 && nop_cnt < 256 {
                let x = mem.read_u32(addr);

                if x == 0x00000000 || x == 0x00000020 {
                    nop_cnt += 1;
                } else {
                    nop_cnt = 0;
                }

                mapping.insert(addr, (x, disassemble(x)));
                addr += 4;
            }
        }

        mapping
    }

    fn capture_running(&self) -> bool {
        super::looper::is_running()
    }

    fn capture_can_use_jit(&self) -> bool {
        match self.exec {
            Executor::ExInterpreter(_) => false,
            Executor::ExJit(_) => true,
        }
    }
}

fn js_array_numbers<'a, 'b, C: Context<'a>>(
    cx: &mut C,
    iter: impl Iterator<Item = &'b u32>,
) -> JsResult<'a, JsArray> {
    let size_hint = iter.size_hint();
    let len = size_hint.1.unwrap_or(size_hint.0);
    let a = JsArray::new(cx, len as u32);

    for (i, s) in iter.enumerate() {
        let v = cx.number(*s);
        a.set(cx, i as u32, v)?;
    }

    Ok(a)
}
