use super::error::AssemblerError;
use super::instruction::Instruction;
use super::segment::Segment;
use crate::assembler::instruction::{FormatR, Register};
use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::str::FromStr;

fn try_parse_number(text: &str) -> Option<u32> {
    let text = text.to_ascii_lowercase();

    if let Some(x) = text.strip_prefix("0x") {
        u32::from_str_radix(x, 16).ok()
    } else if let Some(x) = text.strip_prefix("0o") {
        u32::from_str_radix(x, 8).ok()
    } else if let Some(x) = text.strip_prefix("0b") {
        u32::from_str_radix(x, 2).ok()
    } else {
        u32::from_str(&text).ok()
    }
}

fn try_parse_reg(text: &str) -> Result<u8, AssemblerError> {
    let stripped = text
        .strip_prefix('$')
        .ok_or_else(|| AssemblerError::InvalidRegisterName(text.into()))?;

    // Try parsing as numeric form (e.g. $4)
    if let Ok(x) = u8::from_str(stripped) {
        if x < 32 {
            return Ok(x);
        }
    }

    // Try parsing as textual form (e.g. $v1)
    match stripped {
        "r0" | "zero" => Ok(0),
        "at" => Ok(1),
        "v0" => Ok(2),
        "a0" => Ok(4),
        "a1" => Ok(5),
        "a2" => Ok(6),
        "a3" => Ok(7),
        "t0" => Ok(8),
        "t1" => Ok(9),
        "t2" => Ok(10),
        "t3" => Ok(11),
        "t4" => Ok(12),
        "t5" => Ok(13),
        "t6" => Ok(14),
        "t7" => Ok(15),
        "s0" => Ok(16),
        "s1" => Ok(17),
        "s2" => Ok(18),
        "s3" => Ok(19),
        "s4" => Ok(20),
        "s5" => Ok(21),
        "s6" => Ok(22),
        "s7" => Ok(23),
        "t8" => Ok(24),
        "t9" => Ok(25),
        "k0" => Ok(26),
        "k1" => Ok(27),
        "gp" => Ok(28),
        "sp" => Ok(29),
        "s8" => Ok(30),
        "ra" => Ok(31),
        _ => Err(AssemblerError::InvalidRegisterName(text.into())),
    }
}

fn bail_trailing_token<'a>(mut iter: impl Iterator<Item = &'a str>) -> Result<(), AssemblerError> {
    match iter.next() {
        Some(x) => Err(AssemblerError::TrailingToken(x.into())),
        None => Ok(()),
    }
}

fn try_parse_3arg<'a>(
    line: &'a str,
    mut args: impl Iterator<Item = &'a str>,
) -> Result<FormatR, AssemblerError> {
    let rd = args
        .next()
        .ok_or_else(|| AssemblerError::InvalidNumberOfOperands(line.into()))?;
    let rs = args
        .next()
        .ok_or_else(|| AssemblerError::InvalidNumberOfOperands(line.into()))?;
    let rt = args
        .next()
        .ok_or_else(|| AssemblerError::InvalidNumberOfOperands(line.into()))?;
    bail_trailing_token(args)?;

    Ok(FormatR {
        rd: Register(try_parse_reg(rd.trim())?),
        rs: Register(try_parse_reg(rs.trim())?),
        rt: Register(try_parse_reg(rt.trim())?),
        shamt: 0,
    })
}

fn try_parse_ins<'a>(line: &'a str, mnemonic: &'a str) -> Result<Instruction, AssemblerError> {
    let args = line
        .strip_prefix(mnemonic)
        .expect("line should start with mnemonic");
    let args = args.split(',');

    Ok(match mnemonic {
        "and" => Instruction::And(try_parse_3arg(line, args)?),
        "or" => Instruction::Or(try_parse_3arg(line, args)?),
        "add" => Instruction::Add(try_parse_3arg(line, args)?),
        "sub" => Instruction::Sub(try_parse_3arg(line, args)?),
        "slt" => Instruction::Slt(try_parse_3arg(line, args)?),
        _ => return Err(AssemblerError::UnknownInstruction(mnemonic.into())),
    })
}

#[allow(dead_code)]
pub fn assemble(asm: &str) -> Result<Vec<Segment>, AssemblerError> {
    let mut segs = vec![];
    let mut curr_seg = None;
    let mut global_labels = HashSet::new();
    let mut is_text_seg = false;

    const TEXT_SEGMENT: RangeInclusive<u32> = 0x00400000..=0x0fffffff;
    const DATA_SEGMENT: RangeInclusive<u32> = 0x10000000..=0x7fffffff;

    let mut next_data_addr = 0x10000000;
    let mut next_text_addr = 0x00400000;

    for line in asm.lines() {
        let mut line = line.trim();

        if let Some(comment_pos) = line.find('#') {
            line = &line[..comment_pos];
        }

        if line.is_empty() {
            continue;
        } else if line.len() > 8192 {
            return Err(AssemblerError::LineTooLong);
        }

        let mut tokens = line.split_whitespace();

        // unwrap safety: trimmed and non-empty (thus contains at least one non-whitespace character)
        let first_token = tokens.next().unwrap();

        if first_token == ".text" || first_token == ".data" {
            if let Some(x) = curr_seg {
                segs.push(x);
            }

            let base_addr = match tokens.next().and_then(try_parse_number) {
                Some(x) => x,
                None => {
                    if first_token == ".text" {
                        next_text_addr
                    } else {
                        next_data_addr
                    }
                }
            };

            let seg_type;
            if first_token == ".text" {
                is_text_seg = true;
                seg_type = Some(TEXT_SEGMENT);
            } else if first_token == ".data" {
                is_text_seg = false;
                seg_type = Some(DATA_SEGMENT);
            } else {
                seg_type = None;
            }

            if let Some(x) = seg_type {
                if !x.contains(&base_addr) {
                    return Err(AssemblerError::BaseAddressOutOfRange(base_addr, x));
                }
            }

            curr_seg = Some(Segment::new(base_addr));
            bail_trailing_token(tokens)?;
        } else if first_token == ".globl" {
            let label = tokens.next().ok_or(AssemblerError::RequiredArgNotFound)?;

            if curr_seg.is_none() {
                return Err(AssemblerError::SegmentRequired(line.into()));
            }

            global_labels.insert(label.to_owned());
        } else if first_token == ".word" {
            let seg = curr_seg
                .as_mut()
                .ok_or_else(|| AssemblerError::SegmentRequired(line.into()))?;

            let values = line
                .strip_prefix(first_token)
                .expect("line should start with first token")
                .split(',')
                .map(|x| {
                    try_parse_number(x.trim()).ok_or_else(|| AssemblerError::InvalidToken(x.into()))
                });

            for num in values {
                seg.data.extend_from_slice(&num?.to_ne_bytes());
            }

            if is_text_seg {
                next_text_addr = u32::max(next_text_addr, seg.base_addr + seg.data.len() as u32);
            } else {
                next_data_addr = u32::max(next_data_addr, seg.base_addr + seg.data.len() as u32);
            }
        } else {
            let ins = try_parse_ins(line, first_token)?;
            let seg = curr_seg
                .as_mut()
                .ok_or_else(|| AssemblerError::SegmentRequired(line.into()))?;

            seg.data.extend_from_slice(&ins.encode().to_ne_bytes());

            if is_text_seg {
                next_text_addr = u32::max(next_text_addr, seg.base_addr + seg.data.len() as u32);
            } else {
                next_data_addr = u32::max(next_data_addr, seg.base_addr + seg.data.len() as u32);
            }
        }
    }

    if let Some(x) = curr_seg {
        segs.push(x);
    }

    Ok(segs)
}

#[cfg(test)]
mod test {
    use super::*;
    use byteorder::{NativeEndian, ReadBytesExt};
    use std::io::Cursor;

    #[test]
    fn assemble_empty() {
        assert_eq!(assemble("").unwrap().is_empty(), true);
        assert_eq!(assemble("\n\n\n\n\n\n").unwrap().is_empty(), true);
    }

    #[test]
    fn assemble_empty_seg() {
        assert_eq!(assemble(".text\n.data\n.text\n.data").unwrap().len(), 4);
        assert_eq!(assemble(".data\n.text\n.text\n.text").unwrap().len(), 4);
    }

    #[test]
    fn assemble_arith() {
        let code = ".text\nadd $0, $4, $12\nsub $2, $s0, $zero";
        let segs = assemble(code).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].base_addr, 0x00400000);
        assert_eq!(segs[0].data.len(), 8);
        assert!(segs[0].labels.is_empty());

        let mut data = Cursor::new(&segs[0].data);
        assert_eq!(data.read_u32::<NativeEndian>().unwrap(), 0x008c0020);
        assert_eq!(data.read_u32::<NativeEndian>().unwrap(), 0x02001022);
    }

    #[test]
    fn assemble_data() {
        let code = ".data\n.word 123, 0x123, 0o123\n.word 0xffffffff";
        let segs = assemble(code).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].base_addr, 0x10000000);
        assert_eq!(segs[0].data.len(), 16);
        assert!(segs[0].labels.is_empty());

        let mut data = Cursor::new(&segs[0].data);
        assert_eq!(data.read_u32::<NativeEndian>().unwrap(), 123);
        assert_eq!(data.read_u32::<NativeEndian>().unwrap(), 0x123);
        assert_eq!(data.read_u32::<NativeEndian>().unwrap(), 0o123);
        assert_eq!(data.read_u32::<NativeEndian>().unwrap(), 0xffffffff);
    }
}
