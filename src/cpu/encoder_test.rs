use std::fs::File;
use std::io::{self, Read, Write};
use std::process::Command;
use std::str;
use std::collections::HashMap;
use std::time::Instant;
use std::path::{Path, PathBuf};
use std::iter::FromIterator;

use tempdir::TempDir;
use tera::Context;
use rand::{Rng, XorShiftRng};

use cpu::CPU;
use cpu::encoder::{Encoder, ndisasm_bytes};
use cpu::segment::Segment;
use cpu::parameter::Parameter;
use cpu::instruction::{Instruction, InstructionInfo, RepeatMode};
use cpu::op::Op;
use cpu::register::{R8, R16, AMode, SR};
use memory::mmu::MMU;

#[test] #[ignore] // expensive test
fn can_encode_random_seq() {
    let mut rng = XorShiftRng::new_unseeded();
    let mut code = vec![0u8; 10];

    let mmu = MMU::new();
    let mut cpu = CPU::new(mmu);

    for _ in 0..1000 {
        for mut b in &mut code {
            *b = rng.gen();
        }

        cpu.load_com(&code);

        let encoder = Encoder::new();

        // randomizes a byte sequence and tries to decode the first instruction
        let cs = cpu.get_sr(&SR::CS);
        let ops = cpu.decoder.decode_to_block(cs, 0x100, 1);
        let op = &ops[0];
        if op.instruction.command.is_valid() {
            // - if successful, try to encode. all valid decodings should be mapped for valid
            //   encoding for implemented ops (this should find all missing cases)
            let try_enc = encoder.encode(&op.instruction);
            match try_enc {
                Ok(enc) => {
                    let in_bytes = Vec::from_iter(code[0..enc.len()].iter().cloned());
                    if enc != in_bytes {
                        let ndisasm_of_input = ndisasm_bytes(&in_bytes).unwrap();
                        let ndisasm_of_encode = ndisasm_bytes(&enc).unwrap();
                        if ndisasm_of_input != ndisasm_of_encode {
                            panic!("encoding resulted in wrong sequence.\n\ninput  {:?}\noutput {:?}\ninstr {:?}\nndisasm of\ninput '{}'\nencode '{}'",
                                hex_bytes(&in_bytes),
                                hex_bytes(&enc),
                                op.instruction,
                                ndisasm_of_input,
                                ndisasm_of_encode);
                        }
                    }

                    // - if encode was successful, try to decode that seq again and make sure the resulting
                    //   ops are the same (this should ensure all cases code 2-way to the same values)
                    cpu.load_com(&enc);
                    let decoded = cpu.decoder.decode_to_block(cs, 0x100, 1);
                    let reencoded_op = &decoded[0];
                    if op != reencoded_op {
                        panic!("re-encoding failed: expected {:?}, got {:?}", op, reencoded_op);
                    }
                }
                _ => {
                    // NOTE: commented out for now because encoder.rs handles so few instructions
                    // println!("ERROR: found unsuccessful encode for {:?}: reason {:?}", op, try_enc);
                }
            }
        } else {
            // println!("NOTICE: skipping invalid sequence: {:?}: {}", code, op);
        }
    }
}

#[test]
fn can_encode_inc() {
    let op = Instruction::new1(Op::Inc8, Parameter::Reg8(R8::BH));
    assert_encdec(&op, "inc bh", vec!(0xFE, 0xC7));

    let op = Instruction::new1(Op::Inc8, Parameter::Ptr8AmodeS8(Segment::Default, AMode::BP, 0x10));
    assert_encdec(&op, "inc byte [bp+0x10]", vec!(0xFE, 0x46, 0x10));
    
    let op = Instruction::new1(Op::Inc16, Parameter::Reg16(R16::BX));
    assert_encdec(&op, "inc bx", vec!(0x43));

    let op = Instruction::new1(Op::Inc16, Parameter::Ptr16AmodeS8(Segment::Default, AMode::BP, 0x10));
    assert_encdec(&op, "inc word [bp+0x10]", vec!(0xFF, 0x46, 0x10));
}

#[test]
fn can_encode_dec() {
    let op = Instruction::new1(Op::Dec8, Parameter::Reg8(R8::BH));
    assert_encdec(&op, "dec bh", vec!(0xFE, 0xCF));

    let op = Instruction::new1(Op::Dec16, Parameter::Reg16(R16::BX));
    assert_encdec(&op, "dec bx", vec!(0x4B));

    let op = Instruction::new1(Op::Dec16, Parameter::Ptr16AmodeS8(Segment::Default, AMode::BP, 0x10));
    assert_encdec(&op, "dec word [bp+0x10]", vec!(0xFF, 0x4E, 0x10));
}

#[test]
fn can_encode_push() {
    let op = Instruction::new1(Op::Push16, Parameter::Imm16(0x8088));
    assert_encdec(&op, "push word 0x8088", vec!(0x68, 0x88, 0x80));
}

#[test]
fn can_encode_pop() {
    let op = Instruction::new(Op::Popf);
    assert_encdec(&op, "popf", vec!(0x9D));
}

#[test]
fn can_encode_bitshift_instructions() {
    let op = Instruction::new2(Op::Shr8, Parameter::Reg8(R8::AH), Parameter::Imm8(0xFF));
    assert_encdec(&op, "shr ah,byte 0xff", vec!(0xC0, 0xEC, 0xFF));

    let op = Instruction::new2(Op::Shl8, Parameter::Reg8(R8::AH), Parameter::Imm8(0xFF));
    assert_encdec(&op, "shl ah,byte 0xff", vec!(0xC0, 0xE4, 0xFF));
}

#[test]
fn can_encode_int() {
    let op = Instruction::new1(Op::Int(), Parameter::Imm8(0x21));
    assert_encdec(&op, "int 0x21", vec!(0xCD, 0x21));
}

#[test]
fn can_encode_mov_addressing_modes() {
    // r8, imm8
    let op = Instruction::new2(Op::Mov8, Parameter::Reg8(R8::BH), Parameter::Imm8(0xFF));
    assert_encdec(&op, "mov bh,0xff", vec!(0xB7, 0xFF));

    // r16, imm16
    let op = Instruction::new2(Op::Mov16, Parameter::Reg16(R16::BX), Parameter::Imm16(0x8844));
    assert_encdec(&op, "mov bx,0x8844", vec!(0xBB, 0x44, 0x88));

    // r/m8, r8  (dst is r8)
    let op = Instruction::new2(Op::Mov8, Parameter::Reg8(R8::BH), Parameter::Reg8(R8::DL));
    assert_encdec(&op, "mov bh,dl", vec!(0x88, 0xD7));

    // r/m8, r8  (dst is AMode::BP + imm8)
    let op = Instruction::new2(Op::Mov8, Parameter::Ptr8AmodeS8(Segment::Default, AMode::BP, 0x10), Parameter::Reg8(R8::BH));
    assert_encdec(&op, "mov [bp+0x10],bh", vec!(0x88, 0x7E, 0x10));

    // r/m8, r8  (dst is AMode::BP + imm8)    - reversed
    let op = Instruction::new2(Op::Mov8, Parameter::Reg8(R8::BH), Parameter::Ptr8AmodeS8(Segment::Default, AMode::BP, 0x10));
    assert_encdec(&op, "mov bh,[bp+0x10]", vec!(0x8A, 0x7E, 0x10));

    // r8, r/m8
    let op = Instruction::new2(Op::Mov8, Parameter::Reg8(R8::BH), Parameter::Ptr8(Segment::Default, 0xC365));
    assert_encdec(&op, "mov bh,[0xc365]", vec!(0x8A, 0x3E, 0x65, 0xC3));

    // r/m8, r8  (dst is AMode::BP + imm8)
    let op = Instruction::new2(Op::Mov8, Parameter::Ptr8AmodeS16(Segment::Default, AMode::BP, -0x800), Parameter::Reg8(R8::BH));
    assert_encdec(&op, "mov [bp-0x800],bh", vec!(0x88, 0xBE, 0x00, 0xF8));

    // r/m8, r8  (dst is [imm16]) // XXX no direct amode mapping in resulting Instruction. can we implement a "Instruction.AMode() -> AMode" ?
    let op = Instruction::new2(Op::Mov8, Parameter::Ptr8(Segment::Default, 0x8000), Parameter::Reg8(R8::BH));
    assert_encdec(&op, "mov [0x8000],bh", vec!(0x88, 0x3E, 0x00, 0x80));

    // r/m8, r8  (dst is [bx])
    let op = Instruction::new2(Op::Mov8, Parameter::Ptr8Amode(Segment::Default, AMode::BX), Parameter::Reg8(R8::BH));
    assert_encdec(&op, "mov [bx],bh", vec!(0x88, 0x3F));
}

fn assert_encdec(op :&Instruction, expected_ndisasm: &str, expected_bytes: Vec<u8>) {
    let encoder = Encoder::new();
    let code = encoder.encode(&op).unwrap();

    // decode result and verify with input op
    let mmu = MMU::new();
    let mut cpu = CPU::new(mmu);
    cpu.load_com(&code);
    let cs = cpu.get_sr(&SR::CS);
    let ops = cpu.decoder.decode_to_block(cs, 0x100, 1);
    let decoded_op = &ops[0].instruction;
    assert_eq!(op, decoded_op);

    // verify encoded byte sequence with expected bytes
    assert_eq!(expected_bytes, code);

    // disasm encoded byte sequence and verify with expected ndisasm output
    assert_eq!(expected_ndisasm.to_owned(), ndisasm_bytes(&code).unwrap());
}

fn hex_bytes(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for &b in data {
        write!(&mut s, "{:02x} ", b).expect("Unable to write");
    }
    s.trim().to_owned()
}
