#[allow(unused_imports)]

use std::{mem, u8};
use std::num::Wrapping;

use cpu::flags::Flags;
use cpu::instruction::{Instruction, InstructionInfo, ModRegRm, RepeatMode};
use cpu::parameter::{Parameter, ParameterSet};
use cpu::op::{Op, InvalidOp};
use cpu::register::{Register16, R8, R16, SR, AMode};
use cpu::decoder::Decoder;
use cpu::segment::Segment;
use memory::Memory;
use memory::mmu::MMU;
use interrupt;
use gpu::GPU;
use pit::PIT;

#[cfg(test)]
#[path = "./cpu_test.rs"]
mod cpu_test;

#[derive(Debug)]
enum Exception {
    // http://wiki.osdev.org/Interrupt_Vector_Table
    DIV0 = 0,    // Divide by 0
    UD = 6,      // Invalid opcode (UD2)
    DF = 8,      // Double fault
    TS = 10,     // Invalid TSS
    NP = 11,     // Segment not present
    SS = 12,     // Stack-segment fault
    GP = 13,     // General protection fault
    PF = 14,     // Page fault
}

#[derive(Clone, Default)]
pub struct CPU {
    pub ip: u16,
    pub instruction_count: usize,
    pub cycle_count: usize,
    pub mmu: MMU,
    pub r16: [Register16; 8], // general purpose registers
    pub sreg16: [u16; 6], // segment registers
    pub flags: Flags,
    pub gpu: GPU,
    pub pit: PIT,
    rom_base: usize,
    pub fatal_error: bool, // for debugging: signals to debugger we hit an error
    pub deterministic: bool, // for testing: toggles non-deterministic behaviour
    pub decoder: Decoder,
    pub clock_hz: usize,
}

impl CPU {
    pub fn new(mmu: MMU) -> Self {
        CPU {
            ip: 0,
            instruction_count: 0,
            cycle_count: 0,
            r16: [Register16 { val: 0 }; 8],
            sreg16: [0; 6],
            flags: Flags::new(),
            gpu: GPU::new(),
            pit: PIT::new(),
            rom_base: 0,
            fatal_error: false,
            deterministic: false,
            mmu: mmu.clone(),
            decoder: Decoder::new(mmu),
            clock_hz: 5_000_000, // Intel 8086: 0.330 MIPS at 5.000 MHz
        }
    }

    // reset the CPU but keep the memory
    pub fn soft_reset(&mut self) {
        let cpu = CPU::new(self.mmu.clone());
        *self = cpu;
    }

    // reset the CPU and memory
    pub fn hard_reset(&mut self, mmu: MMU) {
        let cpu = CPU::new(mmu);
        *self = cpu;
    }

    /*
    pub fn load_bios(&mut self, data: &[u8]) {
        self.sreg16[CS] = 0xF000;
        self.ip = 0x0000;
        let end = self.ip + data.len() as u16;
        println!("loading bios to {:06X}..{:06X}", self.ip, end);
        self.rom_base = self.ip as usize;
        self.mmu.write(self.sreg16[CS], self.ip, data);
    }
    */

    // load .com program into CS:0100 and set IP to program start
    pub fn load_com(&mut self, data: &[u8]) {
        // CS,DS,ES,SS = PSP segment
        let psp_segment = 0x085F; // is what dosbox used
        self.set_sr(&SR::CS, psp_segment);
        self.set_sr(&SR::DS, psp_segment);
        self.set_sr(&SR::ES, psp_segment);
        self.set_sr(&SR::SS, psp_segment);

        // offset of last word available in first 64k segment
        self.set_r16(&R16::SP, 0xFFFE);
        self.set_r16(&R16::BP, 0x091C); // is what dosbox used

        // This is what dosbox initializes the registers to
        // at program load
        self.set_r16(&R16::CX, 0x00FF);
        self.set_r16(&R16::DX, psp_segment);
        self.set_r16(&R16::SI, 0x0100);
        self.set_r16(&R16::DI, 0xFFFE);

        self.ip = 0x0100;
        let min = self.get_address();
        self.rom_base = min;

        let cs = self.get_sr(&SR::CS);
        self.mmu.write(cs, self.ip, data);
    }

    pub fn get_r16(&self, r: &R16) -> u16 {
        match r {
            &R16::AX => self.r16[0].val,
            &R16::CX => self.r16[1].val,
            &R16::DX => self.r16[2].val,
            &R16::BX => self.r16[3].val,
            &R16::SP => self.r16[4].val,
            &R16::BP => self.r16[5].val,
            &R16::SI => self.r16[6].val,
            &R16::DI => self.r16[7].val,
        }
    }

    pub fn set_r16(&mut self, r: &R16, val: u16) {
        self.r16[r.index()].val = val;
    }

    pub fn set_sr(&mut self, sr: &SR, val: u16) {
        self.sreg16[sr.index()] = val;
    }

    pub fn get_sr(&self, sr: &SR) -> u16 {
         match sr {
            &SR::ES => self.sreg16[0],
            &SR::CS => self.sreg16[1],
            &SR::SS => self.sreg16[2],
            &SR::DS => self.sreg16[3],
            &SR::FS => self.sreg16[4],
            &SR::GS => self.sreg16[5],
        }
    }

    pub fn get_r8(&self, r: R8) -> u8 {
        match r {
            R8::AL => self.r16[0].lo_u8(),
            R8::CL => self.r16[1].lo_u8(),
            R8::DL => self.r16[2].lo_u8(),
            R8::BL => self.r16[3].lo_u8(),
            R8::AH => self.r16[0].hi_u8(),
            R8::CH => self.r16[1].hi_u8(),
            R8::DH => self.r16[2].hi_u8(),
            R8::BH => self.r16[3].hi_u8(),
        }
    }

    pub fn set_r8(&mut self, r: R8, val: u8) {
        match r {
            R8::AL => self.r16[0].set_lo(val),
            R8::CL => self.r16[1].set_lo(val),
            R8::DL => self.r16[2].set_lo(val),
            R8::BL => self.r16[3].set_lo(val),
            R8::AH => self.r16[0].set_hi(val),
            R8::CH => self.r16[1].set_hi(val),
            R8::DH => self.r16[2].set_hi(val),
            R8::BH => self.r16[3].set_hi(val),
        }
    }

    // base address the rom was loaded to
    pub fn get_rom_base(&self) -> usize {
        self.rom_base
    }

    // executes enough instructions that can run for 1 video frame
    pub fn execute_frame(&mut self) {
        let fps = 60;
        let cycles = self.clock_hz / fps;
        // println!("will execute {} cycles", cycles);

        loop {
            self.execute_instruction();
            if self.fatal_error {
                break;
            }
            if self.cycle_count > cycles {
                self.cycle_count = 0;
                break;
            }
        }
    }

    pub fn execute_instruction(&mut self) {
        let cs = self.get_sr(&SR::CS);
        let ip = self.ip;
        let op = self.decoder.get_instruction(Segment::DS, cs, ip);

        match op.command {
            Op::Unknown() => {
                self.fatal_error = true;
                println!("executed unknown op, stopping. {} instructions executed",
                         self.instruction_count);
            }
            Op::Invalid(reason) => {
                self.fatal_error = true;
                match reason {
                    InvalidOp::Op(ops) => {
                        let mut ops_str = String::new();
                        for x in ops {
                            let hex = format!("{:02X} ", x);
                            ops_str.push_str(&hex);
                        }
                        println!("Error unhandled OP {}at {:04X}:{:04X}", ops_str, cs, ip);
                    }
                    InvalidOp::Reg(reg) => {
                        println!("Error invalid register {:02X} at {:04X}:{:04X}", reg, cs, ip);
                    }
                }
                println!("{} Instructions executed", self.instruction_count);
            }
            _ => self.execute(&op),
        }

        // XXX need instruction timing to do this properly
        if self.cycle_count % 100 == 0 {
            self.gpu.progress_scanline();
        }

        if self.cycle_count % 100 == 0 {
            // FIXME: counter should decrement ~18.2 times/sec
            self.pit.counter0.dec();
        }
    }

    fn execute(&mut self, op: &Instruction) {
        let start_ip = self.ip;
        self.ip += u16::from(op.length);
        self.instruction_count += 1;
        self.cycle_count += 1; // XXX temp hack; we pretend each instruction takes 8 cycles due to lack of timing
        match op.command {
            Op::Aaa() => {
                // ASCII Adjust After Addition
                let v = if self.get_r8(R8::AL) > 0xf9 {
                    2
                 } else {
                    1
                };
                self.adjb(6, v);
            }
            Op::Aam() => {
                // ASCII Adjust AX After Multiply
                // tempAL ← AL;
                // AH ← tempAL / imm8; (* imm8 is set to 0AH for the AAM mnemonic *)
                // AL ← tempAL MOD imm8;
                let imm8 = self.read_parameter_value(&op.params.dst) as u8;
                let al = self.get_r8(R8::AL);
                self.set_r8(R8::AH, al / imm8);
                self.set_r8(R8::AL, al % imm8);
            }
            Op::Aas() => {
                // ASCII Adjust AL After Subtraction
                let v = if self.get_r8(R8::AL) < 6 {
                    -2
                } else {
                    -1
                };
                self.adjb(-6, v);
            }
            Op::Adc8() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let carry = if self.flags.carry { 1 } else { 0 };
                let res = (Wrapping(dst) + Wrapping(src) + Wrapping(carry)).0;
                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);

                // The OF, SF, ZF, AF, CF, and PF flags are set according to the result.
                self.flags.set_overflow_add_u8(res, src + carry, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src + carry, dst);
                self.flags.set_carry_u8(res);
                self.flags.set_parity(res);
            }
            Op::Adc16() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let carry = if self.flags.carry { 1 } else { 0 };
                let res = (Wrapping(dst) + Wrapping(src) + Wrapping(carry)).0;
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);

                // The OF, SF, ZF, AF, CF, and PF flags are set according to the result.
                self.flags.set_overflow_add_u16(res, src + carry, dst);
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_auxiliary(res, src + carry, dst);
                self.flags.set_carry_u16(res);
                self.flags.set_parity(res);
            }
            Op::Add8 => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src) as u8;
                let dst = self.read_parameter_value(&op.params.dst) as u8;
                let res = src as usize + dst as usize;
                self.flags.set_carry_u8(res);
                self.flags.set_parity(res);
                self.flags.set_auxiliary(res, src as usize, dst as usize);
                self.flags.set_zero_u8(res);
                self.flags.set_sign_u8(res);
                self.flags.set_overflow_add_u8(res, src as usize, dst as usize);
                self.write_parameter_u8(&op.params.dst, res as u8);
            }
            Op::Add16 => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src) as u16;
                let dst = self.read_parameter_value(&op.params.dst) as u16;
                let res = src as usize + dst as usize;
                self.flags.set_carry_u16(res);
                self.flags.set_parity(res);
                self.flags.set_auxiliary(res, src as usize, dst as usize);
                self.flags.set_zero_u16(res);
                self.flags.set_sign_u16(res);
                self.flags.set_overflow_add_u16(res, src as usize, dst as usize);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, res as u16);
            }
            Op::And8() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst & src;

                // The OF and CF flags are cleared; the SF, ZF, and PF flags
                // are set according to the result.
                self.flags.overflow = false;
                self.flags.carry = false;
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_parity(res);
                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::And16() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst & src;

                // The OF and CF flags are cleared; the SF, ZF, and PF flags
                // are set according to the result.
                self.flags.overflow = false;
                self.flags.carry = false;
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_parity(res);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            Op::Arpl() => {
                // Adjust RPL Field of Segment Selector
                println!("XXX impl {}", op);
                /*
                // NOTE: RPL is the low two bits of the address
                let src = self.read_parameter_value(&op.params.src);
                let mut dst = self.read_parameter_value(&op.params.dst);
                if dst & 3 < src & 3 {
                    self.flags.zero = true;
                    dst = (dst & 0xFFFC) + (src & 3);
                    self.write_parameter_u16(op.segment, &op.params.dst, (dst & 0xFFFF) as u16);
                } else {
                    self.flags.zero = false;
                }
                */
            }
            Op::Bsf => {
                // Bit Scan Forward
                let mut src = self.read_parameter_value(&op.params.src);
                if src == 0 {
                    self.flags.zero = true;
                } else {
                    let mut count = 0;
                    while src & 1 == 0 {
                        count += 1;
                        src >>= 1;
                    }
                    self.write_parameter_u16(op.segment_prefix, &op.params.dst, count);
                    self.flags.zero = false;
                }
            }
            Op::Bt => {
                // Bit Test
                let bit_base = self.read_parameter_value(&op.params.dst);
                let bit_offset = self.read_parameter_value(&op.params.src);
                self.flags.carry = bit_base & (1 << (bit_offset & 15)) != 0;
            }
            Op::Bound() => {
                println!("XXX impl {}", op);
            }
            Op::CallNear() => {
                // call near rel
                let old_ip = self.ip;
                let temp_ip = self.read_parameter_value(&op.params.dst);
                self.push16(old_ip);
                self.ip = temp_ip as u16;
            }
            Op::Cbw() => {
                // Convert Byte to Word
                let ah = if self.get_r8(R8::AL) & 0x80 != 0 {
                    0xFF
                } else {
                    0x00
                };
                self.set_r8(R8::AH, ah);
            }
            Op::Clc() => {
                // Clear Carry Flag
                self.flags.carry = false;
            }
            Op::Cld() => {
                // Clear Direction Flag
                self.flags.direction = false;
            }
            Op::Cli() => {
                // Clear Interrupt Flag
                self.flags.interrupt = false;
            }
            Op::Cmc() => {
                // Complement Carry Flag
                self.flags.carry = !self.flags.carry;
            }
            Op::Cmp8() => {
                // two parameters
                // Modify status flags in the same manner as the SUB instruction
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) - Wrapping(src)).0;

                // The CF, OF, SF, ZF, AF, and PF flags are set according to the result.
                self.flags.set_carry_u8(res);
                self.flags.set_overflow_sub_u8(res, src, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
            }
            Op::Cmp16() => {
                // two parameters
                // Modify status flags in the same manner as the SUB instruction
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                self.cmp16(dst, src);
            }
            Op::Cmpsw() => {
                // no parameters
                // Compare word at address DS:(E)SI with word at address ES:(E)DI
                // The DS segment may be overridden with a segment override prefix, but the ES segment cannot be overridden.
                let src = self.mmu.read_u16(self.segment(op.segment_prefix), self.get_r16(&R16::SI)) as usize;
                let dst = self.mmu.read_u16(self.get_sr(&SR::ES), self.get_r16(&R16::DI)) as usize;
                self.cmp16(dst, src);
                println!("XXX Cmpsw - verify implementation");
            }
            Op::Cwd() => {
                // Convert Word to Doubleword
                // DX:AX ← sign-extend of AX.
                let dx = if self.get_r16(&R16::AX) & 0x8000 != 0 {
                    0xFFFF
                } else {
                    0
                };
                self.set_r16(&R16::DX, dx);
            }
            Op::Daa() => {
                // Decimal Adjust AL after Addition
                self.adj4(6, 0x60);
            }
            Op::Das() => {
                // Decimal Adjust AL after Subtraction
                self.adj4(-6, -0x60);
            }
            Op::Dec8() => {
                // single parameter (dst)
                let dst = self.read_parameter_value(&op.params.dst);
                let src = 1;
                let res = (Wrapping(dst) - Wrapping(src)).0;

                // The CF flag is not affected. The OF, SF, ZF, AF,
                // and PF flags are set according to the result.
                self.flags.set_overflow_sub_u8(res, src, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);

                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::Dec16() => {
                // single parameter (dst)
                let dst = self.read_parameter_value(&op.params.dst);
                let src = 1;
                let res = (Wrapping(dst) - Wrapping(src)).0;

                // The CF flag is not affected. The OF, SF, ZF, AF,
                // and PF flags are set according to the result.
                self.flags.set_overflow_sub_u16(res, src, dst);
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);

                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            Op::Div8() => {
                let dst = self.get_r16(&R16::AX) as usize;
                let src = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) / Wrapping(src)).0;
                let rem = (Wrapping(dst) % Wrapping(src)).0;
                // The CF, OF, SF, ZF, AF, and PF flags are undefined.
                self.set_r8(R8::AL, (res & 0xFF) as u8); // quotient
                self.set_r8(R8::AH, (rem & 0xFF) as u8); // remainder
            }
            Op::Div16() => {
                let dst = ((self.get_r16(&R16::DX) as usize) << 16) + self.get_r16(&R16::AX) as usize; // DX:AX
                let src = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) / Wrapping(src)).0;
                let rem = (Wrapping(dst) % Wrapping(src)).0;
                // The CF, OF, SF, ZF, AF, and PF flags are undefined.
                self.set_r16(&R16::AX, (res & 0xFFFF) as u16); // quotient
                self.set_r16(&R16::DX, (rem & 0xFFFF) as u16); // remainder
            }
            Op::Enter => {
                // Make Stack Frame for Procedure Parameters
                // Create a stack frame with optional nested pointers for a procedure.
                // XXX test this
                let alloc_size = self.read_parameter_value(&op.params.dst) as u16;
                let mut nesting_level = self.read_parameter_value(&op.params.src);

                nesting_level &= 0x1F; // XXX "mod 32" says docs
                let bp = self.get_r16(&R16::BP);
                self.push16(bp);
                let frame_temp = self.get_r16(&R16::SP);

                if nesting_level != 0 {
                    for i in 0..nesting_level {
                        let bp = self.get_r16(&R16::BP) - 2;
                        self.set_r16(&R16::BP, bp);
                        let val = self.mmu.read_u16(self.get_sr(&SR::SS), self.get_r16(&R16::BP));
                        println!("XXX ENTER: pushing {} = {:04X}", i, val);
                        self.push16(val);
                    }
                    self.push16(frame_temp);
                }

                self.set_r16(&R16::BP, frame_temp);
                let sp = self.get_r16(&R16::SP) - alloc_size;
                self.set_r16(&R16::SP, sp);
            }
            Op::Hlt() => {
                // println!("XXX impl {}", op);
                // self.fatal_error = true;
            }
            Op::Idiv8 => {
                let dividend = self.get_r16(&R16::AX) as i16;
                let op1 = self.read_parameter_value(&op.params.dst) as i16;
                if op1 == 0 {
                    self.exception(Exception::DIV0, 0);
                }
                let quo = dividend / op1;
                let rem = dividend % op1;
                if dividend > 0xFF {
                    self.exception(Exception::DIV0, 0);
                }
                self.set_r8(R8::AL, (quo & 0xFF) as u8);
                self.set_r8(R8::AH, (rem & 0xFF) as u8);
            }
            Op::Idiv16 => {
                let dividend = (((self.get_r16(&R16::DX) as i32) << 16) | self.get_r16(&R16::AX) as i32) as isize; // DX:AX
                let op1 = (self.read_parameter_value(&op.params.dst) as i16) as isize;
                if op1 == 0 {
                    self.exception(Exception::DIV0, 0);
                }
                let quo = dividend / op1;
                let rem = dividend % op1;
                if quo != quo & 0xFFFF {
                    self.exception(Exception::DIV0, 0);
                }
                self.set_r16(&R16::AX, quo as u16);
                self.set_r16(&R16::DX, rem as u16);
            }
            Op::Imul8 => {
                // NOTE: only 1-parameter imul8 instruction exists
                // IMUL r/m8               : AX← AL ∗ r/m byte.
                let f1 = self.get_r8(R8::AL) as i8;
                let f2 = self.read_parameter_value(&op.params.dst) as i8;
                let product = f1 as i16 * f2 as i16;
                self.set_r16(&R16::AX, product as u16);

                // XXX flags
                if self.get_r16(&R16::DX) != 0 {
                    self.flags.carry = true;
                    self.flags.overflow = true;
                } else {
                    self.flags.carry = false;
                    self.flags.overflow = false;
                }
            }
            Op::Imul16 => {
                match op.params.count() {
                    1 => {
                        // IMUL r/m16               : DX:AX ← AX ∗ r/m word.
                        let a = self.read_parameter_value(&op.params.dst) as i16;
                        let tmp = (self.get_r16(&R16::AX) as i16) as isize * a as isize;
                        self.set_r16(&R16::AX, tmp as u16);
                        self.set_r16(&R16::DX, (tmp >> 16) as u16);
                    }
                    2 => {
                        // IMUL r16, r/m16          : word register ← word register ∗ r/m16.
                        let a = self.read_parameter_value(&op.params.dst);
                        let b = self.read_parameter_value(&op.params.src);
                        let tmp = a as isize * b as isize;
                        self.write_parameter_u16(op.segment_prefix, &op.params.dst, (tmp & 0xFFFF) as u16);
                    }
                    3 => {
                        // IMUL r16, r/m16, imm8    : word register ← r/m16 ∗ sign-extended immediate byte.
                        // IMUL r16, r/m16, imm16   : word register ← r/m16 ∗ immediate word.
                        let a = self.read_parameter_value(&op.params.src);
                        let b = self.read_parameter_value(&op.params.src2);
                        let tmp = b as isize * a as isize;
                        self.write_parameter_u16(op.segment_prefix, &op.params.dst, (tmp & 0xFFFF) as u16);
                    }
                    _ => {
                        panic!("imul16 with {} parameters: {}", op.params.count(), op);
                    }
                }

                // XXX flags
                if self.get_r16(&R16::DX) != 0 {
                    self.flags.carry = true;
                    self.flags.overflow = true;
                } else {
                    self.flags.carry = false;
                    self.flags.overflow = false;
                }
            }
            Op::In8() => {
                // Input from Port
                // two parameters (dst=AL)
                let src = self.read_parameter_value(&op.params.src);
                let data = self.in_port(src as u16);
                self.write_parameter_u8(&op.params.dst, data);
            }
            Op::Inc8() => {
                let dst = self.read_parameter_value(&op.params.dst);
                let src = 1;
                let res = (Wrapping(dst) + Wrapping(src)).0;

                // The OF, SF, ZF, AF, and PF flags are set according to the result.
                self.flags.set_overflow_add_u8(res, src, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);

                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::Inc16() => {
                let dst = self.read_parameter_value(&op.params.dst);
                let src = 1;
                let res = (Wrapping(dst) + Wrapping(src)).0;

                // The OF, SF, ZF, AF, and PF flags are set according to the result.
                self.flags.set_overflow_add_u16(res, src, dst);
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);

                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            Op::Insb() => {
                println!("XXX impl {}", op);
            }
            Op::Int() => {
                let int = self.read_parameter_value(&op.params.dst);
                self.int(int as u8);
            }
            Op::Ja() => {
                // Jump if above (CF=0 and ZF=0).    (alias: jnbe)
                if !self.flags.carry & !self.flags.zero {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jc() => {
                // Jump if carry (CF=1).    (alias: jb, jnae)
                if self.flags.carry {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jcxz() => {
                // Jump if CX register is 0.
                if self.get_r16(&R16::CX) == 0 {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jg() => {
                // Jump if greater (ZF=0 and SF=OF).    (alias: jnle)
                if !self.flags.zero & self.flags.sign == self.flags.overflow {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jl() => {
                // Jump if less (SF ≠ OF).    (alias: jnge)
                if self.flags.sign != self.flags.overflow {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::JmpFar() => {
                match op.params.dst {
                    Parameter::Ptr16Imm(seg, imm) => {
                        self.set_sr(&SR::CS, seg);
                        self.ip = imm;
                    }
                    _ => panic!("jmp far with unexpected type {:?}", op.params.dst),
                }
            }
            Op::JmpNear() | Op::JmpShort() => {
                self.ip = self.read_parameter_value(&op.params.dst) as u16;
            }
            Op::Jna() => {
                // Jump if not above (CF=1 or ZF=1).    (alias: jbe)
                if self.flags.carry | self.flags.zero {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jnc() => {
                // Jump if not carry (CF=0).    (alias: jae, jnb)
                if !self.flags.carry {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jng() => {
                // Jump if not greater (ZF=1 or SF ≠ OF).    (alias: jle)
                if self.flags.zero | self.flags.sign != self.flags.overflow {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jnl() => {
                // Jump if not less (SF=OF).    (alias: jge)
                if self.flags.sign == self.flags.overflow {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jns() => {
                // Jump if not sign (SF=0).
                if !self.flags.sign {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jno() => {
                // Jump if not overflow (OF=0).
                if !self.flags.overflow {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jnz() => {
                // Jump if not zero (ZF=0).    (alias: jne)
                if !self.flags.zero {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Js() => {
                // Jump if sign (SF=1).
                if self.flags.sign {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jo() => {
                // Jump if overflow (OF=1).
                if self.flags.overflow {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jpe() => {
                // Jump short if parity even (PF=1)
                if self.flags.parity {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jpo() => {
                // Jump short if parity odd (PF=0).
                 if !self.flags.parity {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Jz() => {
                // Jump if zero (ZF ← 1).    (alias: je)
                if self.flags.zero {
                    self.ip = self.read_parameter_value(&op.params.dst) as u16;
                }
            }
            Op::Lea16() => {
                // Load Effective Address
                let src = self.read_parameter_address(&op.params.src) as u16;
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, src);
            }
            Op::Lds() => {
                // Load DS:r16 with far pointer from memory.
                let seg = self.read_parameter_address(&op.params.src) as u16;
                let val = self.read_parameter_value(&op.params.src) as u16;
                self.set_sr(&SR::DS, seg);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, val);
            }
            Op::Leave => {
                // High Level Procedure Exit
                // Set SP to BP, then pop BP.
                // XXX test this
                let bp = self.get_r16(&R16::BP);
                self.set_r16(&R16::SP, bp);
                let bp = self.pop16();
                self.set_r16(&R16::BP, bp);
            }
            Op::Les() => {
                // les ax, [0x104]
                // Load ES:r16 with far pointer from memory.
                let seg = self.read_parameter_address(&op.params.src) as u16;
                let val = self.read_parameter_value(&op.params.src) as u16;
                self.set_sr(&SR::ES, seg);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, val);
            }
            Op::Lodsb() => {
                // no arguments
                // Load byte at address DS:(E)SI into AL.
                // The DS segment may be over-ridden with a segment override prefix.
                let val = self.mmu.read_u8(self.segment(op.segment_prefix), self.get_r16(&R16::SI));

                self.set_r8(R8::AL, val);
                let si = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::SI)) + Wrapping(1)).0
                } else {
                    (Wrapping(self.get_r16(&R16::SI)) - Wrapping(1)).0
                };
                self.set_r16(&R16::SI, si);
            }
            Op::Lodsw() => {
                // no arguments
                // Load word at address DS:(E)SI into AX.
                // The DS segment may be over-ridden with a segment override prefix.
                let val = self.mmu.read_u16(self.segment(op.segment_prefix), self.get_r16(&R16::SI));

                self.set_r16(&R16::AX, val);
                let si = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::SI)) + Wrapping(2)).0
                } else {
                    (Wrapping(self.get_r16(&R16::SI)) - Wrapping(2)).0
                };
                self.set_r16(&R16::SI, si);
            }
            Op::Loop() => {
                // Decrement count; jump short if count ≠ 0.
                let dst = self.read_parameter_value(&op.params.dst) as u16;
                let cx = (Wrapping(self.get_r16(&R16::CX)) - Wrapping(1)).0;
                self.set_r16(&R16::CX, cx);
                if cx != 0 {
                    self.ip = dst;
                }
            }
            Op::Loope() => {
                // Decrement count; jump short if count ≠ 0 and ZF = 1.
                let dst = self.read_parameter_value(&op.params.dst) as u16;
                let cx = (Wrapping(self.get_r16(&R16::CX)) - Wrapping(1)).0;
                self.set_r16(&R16::CX, cx);
                if cx != 0 && self.flags.zero {
                    self.ip = dst;
                }
            }
            Op::Loopne() => {
                // Decrement count; jump short if count ≠ 0 and ZF = 0.
                let dst = self.read_parameter_value(&op.params.dst) as u16;
                let cx = (Wrapping(self.get_r16(&R16::CX)) - Wrapping(1)).0;
                self.set_r16(&R16::CX, cx);
                if cx != 0 && !self.flags.zero {
                    self.ip = dst;
                }
            } 
            Op::Mov8() => {
                // two arguments (dst=reg)
                let data = self.read_parameter_value(&op.params.src) as u8;
                self.write_parameter_u8(&op.params.dst, data);
            }
            Op::Mov16() => {
                // two arguments (dst=reg)
                let data = self.read_parameter_value(&op.params.src) as u16;
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, data);
            }
            Op::Movsb() => {
                // move byte from address DS:(E)SI to ES:(E)DI.
                // The DS segment may be overridden with a segment override prefix, but the ES segment cannot be overridden.
                let b = self.mmu.read_u8(self.segment(op.segment_prefix), self.get_r16(&R16::SI));
                let si = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::SI)) + Wrapping(1)).0
                } else {
                    (Wrapping(self.get_r16(&R16::SI)) - Wrapping(1)).0
                };
                self.set_r16(&R16::SI, si);
                let es = self.get_sr(&SR::ES);
                let di = self.get_r16(&R16::DI);
                self.mmu.write_u8(es, di, b);
                let di = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::DI)) + Wrapping(1)).0
                } else {
                    (Wrapping(self.get_r16(&R16::DI)) - Wrapping(1)).0
                };
                self.set_r16(&R16::DI, di);
            }
            Op::Movsw() => {
                // move word from address DS:(E)SI to ES:(E)DI.
                // The DS segment may be overridden with a segment override prefix, but the ES segment cannot be overridden.
                let b = self.mmu.read_u16(self.segment(op.segment_prefix), self.get_r16(&R16::SI));
                let si = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::SI)) + Wrapping(2)).0
                } else {
                    (Wrapping(self.get_r16(&R16::SI)) - Wrapping(2)).0
                };
                self.set_r16(&R16::SI, si);
                let es = self.get_sr(&SR::ES);
                let di = self.get_r16(&R16::DI);
                self.mmu.write_u16(es, di, b);
                let di = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::DI)) + Wrapping(2)).0
                } else {
                    (Wrapping(self.get_r16(&R16::DI)) - Wrapping(2)).0
                };
                self.set_r16(&R16::DI, di);
            }
            Op::Movsx16() => {
                // 80386+
                // Move with Sign-Extension
                // moves a signed value into a register and sign-extends it with 1.
                // two arguments (dst=reg)
                let src = self.read_parameter_value(&op.params.src) as u8;

                let mut data = u16::from(src);
                // XXX should not work identical as Movzx16
                if src & 0x80 != 0 {
                    data += 0xFF00;
                }
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, data);
            }
            Op::Movzx16() => {
                // 80386+
                // Move with Zero-Extend
                // moves an unsigned value into a register and zero-extends it with zero.
                // two arguments (dst=reg)
                let src = self.read_parameter_value(&op.params.src) as u8;
                let mut data = u16::from(src);
                if src & 0x80 != 0 {
                    data += 0xFF00;
                }
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, data);
            }
            Op::Mul8() => {
                // Unsigned multiply (AX ← AL ∗ r/m8).
                let src = self.get_r8(R8::AL) as usize;
                let dst = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) * Wrapping(src)).0;

                self.set_r16(&R16::AX, (res & 0xFFFF) as u16);
                // The OF and CF flags are set to 0 if the upper half of the
                // result is 0; otherwise, they are set to 1.
                // The SF, ZF, AF, and PF flags are undefined.
                // XXX flags
            }
            Op::Mul16() => {
                // Unsigned multiply (DX:AX ← AX ∗ r/m16).
                let src = self.get_r16(&R16::AX) as usize;
                let dst = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) * Wrapping(src)).0;

                self.set_r16(&R16::AX, (res & 0xFFFF) as u16);
                self.set_r16(&R16::DX, (res >> 16) as u16);

                let dx_true = self.get_r16(&R16::DX) != 0;
                self.flags.carry = dx_true;
                self.flags.overflow = dx_true;
                // XXX ZF is undefined in later docs
                self.flags.zero = (self.get_r16(&R16::AX) != 0) | (self.get_r16(&R16::DX) != 0);
            }
            Op::Neg8() => {
                // one argument
                let dst = self.read_parameter_value(&op.params.dst);
                let src = 0;
                let res = (Wrapping(src) - Wrapping(dst)).0;
                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);

                // The CF flag set to 0 if the source operand is 0; otherwise it is set to 1.
                if src == 0 {
                    self.flags.carry = false;
                } else {
                    self.flags.carry = true;
                }
                // The OF, SF, ZF, AF, and PF flags are set according to the result.
                self.flags.set_overflow_sub_u8(res, src, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
            }
            Op::Neg16() => {
                // one argument
                let dst = self.read_parameter_value(&op.params.dst);
                let src = 0;
                let res = (Wrapping(src) - Wrapping(dst)).0;
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);

                // The CF flag set to 0 if the source operand is 0; otherwise it is set to 1.
                if src == 0 {
                    self.flags.carry = false;
                } else {
                    self.flags.carry = true;
                }
                // The OF, SF, ZF, AF, and PF flags are set according to the result.
                self.flags.set_overflow_sub_u16(res, src, dst);
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
            }
            Op::Nop() => {}
            Op::Not8() => {
                // one arguments (dst)
                let dst = self.read_parameter_value(&op.params.dst);
                let res = !dst;
                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
                // Flags Affected: None
            }
            Op::Not16() => {
                // one arguments (dst)
                let dst = self.read_parameter_value(&op.params.dst);
                let res = !dst;
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
                // Flags Affected: None
            }
            Op::Or8() => {
                // two arguments (dst=AL)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst | src;
                // The OF and CF flags are cleared; the SF, ZF, and PF flags
                // are set according to the result.
                self.flags.overflow = false;
                self.flags.carry = false;
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_parity(res);
                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::Or16() => {
                // two arguments (dst=AX)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst | src;
                // The OF and CF flags are cleared; the SF, ZF, and PF flags
                // are set according to the result.
                self.flags.overflow = false;
                self.flags.carry = false;
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_parity(res);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            Op::Out8() => {
                // two arguments
                let addr = self.read_parameter_value(&op.params.dst) as u16;
                let val = self.read_parameter_value(&op.params.src) as u8;
                self.out_u8(addr, val);
            }
            Op::Out16() => {
                // two arguments
                let addr = self.read_parameter_value(&op.params.dst) as u16;
                let val = self.read_parameter_value(&op.params.src) as u16;
                self.out_u16(addr, val);
            }
            Op::Outsb() => {
                // Output byte from memory location specified in DS:(E)SI or RSI to I/O port specified in DX.
                // no arguments
                let val = self.mmu.read_u8(self.segment(op.segment_prefix), self.get_r16(&R16::SI));
                let port = self.get_r16(&R16::DX);
                self.out_u8(port, val);
                let si = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::SI)) + Wrapping(1)).0
                } else {
                    (Wrapping(self.get_r16(&R16::SI)) - Wrapping(1)).0
                };
                self.set_r16(&R16::SI, si);
            }
            Op::Outsw() => {
                // Output word from memory location specified in DS:(E)SI or RSI to I/O port specified in DX**.
                // no arguments
                let val = self.mmu.read_u16(self.segment(op.segment_prefix), self.get_r16(&R16::SI));
                let port = self.get_r16(&R16::DX);
                self.out_u16(port, val);
                let si = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::SI)) + Wrapping(2)).0
                } else {
                    (Wrapping(self.get_r16(&R16::SI)) - Wrapping(2)).0
                };
                self.set_r16(&R16::SI, si);
            }
            Op::Pop16() => {
                // one arguments (dst)
                let data = self.pop16();
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, data);
            }
            Op::Popa() => {
                // Pop All General-Purpose Registers
                let di = self.pop16();
                self.set_r16(&R16::DI, di);
                let si = self.pop16();
                self.set_r16(&R16::SI, si);
                let bp = self.pop16();
                self.set_r16(&R16::BP, bp);
                let sp = self.get_r16(&R16::SP) + 2; // skip next word of stack
                self.set_r16(&R16::SP, sp);
                let bx = self.pop16();
                self.set_r16(&R16::BX, bx);
                let dx = self.pop16();
                self.set_r16(&R16::DX, dx);
                let cx = self.pop16();
                self.set_r16(&R16::CX, cx);
                let ax = self.pop16();
                self.set_r16(&R16::AX, ax);
            }
            Op::Popf() => {
                // Pop top of stack into lower 16 bits of EFLAGS.
                let data = self.pop16();
                self.flags.set_u16(data);
            }
            Op::Push16() => {
                // single parameter (dst)
                let data = self.read_parameter_value(&op.params.dst) as u16;
                self.push16(data);
            }
            Op::Pusha() => {
                // Push All General-Purpose Registers
                let ax = self.get_r16(&R16::AX);
                let cx = self.get_r16(&R16::CX);
                let dx = self.get_r16(&R16::DX);
                let bx = self.get_r16(&R16::BX);
                let sp = self.get_r16(&R16::SP);
                let bp = self.get_r16(&R16::BP);
                let si = self.get_r16(&R16::SI);
                let di = self.get_r16(&R16::DI);

                self.push16(ax);
                self.push16(cx);
                self.push16(dx);
                self.push16(bx);
                self.push16(sp);
                self.push16(bp);
                self.push16(si);
                self.push16(di);
            }
            Op::Pushf() => {
                // push FLAGS register onto stack
                let data = self.flags.u16();
                self.push16(data);
            }
            Op::Rcl8 => {
                // Rotate 9 bits (CF, r/m8) left imm8 times.
                // two arguments
                let op1 = self.read_parameter_value(&op.params.dst) as u8;
                let count = (self.read_parameter_value(&op.params.src) & 0x1F) % 9;
                if count > 0 {
                    let cf = self.flags.carry_val() as u8;
                    let res = if count == 1 {
                        (op1 << 1) | cf
                    } else {
                        (op1 << count) | (cf << (count - 1)) | (op1 >> (9 - count))
                    };
                    self.write_parameter_u8(&op.params.dst, res as u8);
                    self.flags.carry = (op1 >> (8 - count)) & 1 != 0;
                    // For left rotates, the OF flag is set to the exclusive OR of the CF bit
                    // (after the rotate) and the most-significant bit of the result.
                    self.flags.overflow = self.flags.carry_val() as u8 ^ (op1 >> 7) != 0;
                }
            }
            Op::Rcl16 => {
                // Rotate 9 bits (CF, r/m8) left imm8 times.
                // two arguments
                let op1 = self.read_parameter_value(&op.params.dst) as u16;
                let count = (self.read_parameter_value(&op.params.src) & 0x1F) % 17;
                if count > 0 {
                    let cf = self.flags.carry_val() as u16;
                    let res = if count == 1 {
                        (op1 << 1) | cf
                    } else if count == 16 {
                        (cf << 15) | (op1 >> 1)
                    } else {
                        (op1 << count) | (cf << (count - 1)) | (op1 >> (17 - count))
                    };
                    self.write_parameter_u16(op.segment_prefix, &op.params.dst, res as u16);
                    self.flags.carry = (op1 >> (16 - count)) & 1 != 0;
                    self.flags.overflow = self.flags.carry_val() as u16 ^ (op1 >> 15) != 0;
                }
            }
            Op::Rcr8 => {
                // two arguments
                // rotate 9 bits right `op1` times
                let op1 = self.read_parameter_value(&op.params.dst);
                let count = (self.read_parameter_value(&op.params.src) as u32 & 0x1F) % 9;
                if count > 0 {
                    let cf = self.flags.carry_val();
                    let res = (op1 >> count) | (cf << (8 - count)) | (op1 << (9 - count));
                    self.write_parameter_u8(&op.params.dst, res as u8);
                    self.flags.carry = (op1 >> (count - 1)) & 1 != 0;
                    // For right rotates, the OF flag is set to the exclusive OR of the
                    // two most-significant bits of the result.
                    let bit7 = (res >> 7) & 1;
                    let bit6 = (res >> 6) & 1;
                    self.flags.overflow = bit7 ^ bit6 != 0;
                }
            }
            Op::Rcr16 => {
                // two arguments
                // rotate 9 bits right `op1` times
                let op1 = self.read_parameter_value(&op.params.dst);
                let count = (self.read_parameter_value(&op.params.src) as u32 & 0x1F) % 17;
                if count > 0 {
                    let cf = self.flags.carry_val();
                    let res = (op1 >> count) | (cf << (16 - count)) | (op1 << (17 - count));
                    self.write_parameter_u16(op.segment_prefix, &op.params.dst, res as u16);
                    self.flags.carry = (op1 >> (count - 1)) & 1 != 0;
                    let bit15 = (res >> 15) & 1;
                    let bit14 = (res >> 14) & 1;
                    self.flags.overflow = bit15 ^ bit14 != 0;
                }
            }
            Op::Retf => {
                if op.params.count() == 1 {
                    // 1 argument: pop imm16 bytes from stack
                    let imm16 = self.read_parameter_value(&op.params.dst) as u16;
                    let sp = self.get_r16(&R16::SP) + imm16;
                    self.set_r16(&R16::SP, sp);
                }
                self.ip = self.pop16();
                let cs = self.pop16();
                self.set_sr(&SR::CS, cs);
            }
            Op::Retn => {
                if op.params.count() == 1 {
                    // 1 argument: pop imm16 bytes from stack
                    let imm16 = self.read_parameter_value(&op.params.dst) as u16;
                    let sp = self.get_r16(&R16::SP) + imm16;
                    self.set_r16(&R16::SP, sp);
                }
                if self.get_r16(&R16::SP) == 0xFFFE {
                    println!("retn called at end of stack, ending program after {} instructions", self.instruction_count);
                    self.fatal_error = true;
                }
                self.ip = self.pop16();
            }
            Op::Rol8 => {
                // Rotate 8 bits of 'dst' left for 'src' times.
                // two arguments
                let mut res = self.read_parameter_value(&op.params.dst) as u8;
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                res = res.rotate_left(count as u32);
                self.write_parameter_u8(&op.params.dst, res);
                let bit0 = res & 1;
                let bit7 = (res >> 7) & 1;
                if count == 1 {
                    self.flags.overflow = bit0 ^ bit7 != 0;
                }
                self.flags.carry = bit0 != 0;
            }
            Op::Rol16 => {
                // Rotate 16 bits of 'dst' left for 'src' times.
                // two arguments
                let mut res = self.read_parameter_value(&op.params.dst) as u16;
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                res = res.rotate_left(count as u32);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, res);
                let bit0 = res & 1;
                let bit15 = (res >> 15) & 1;
                if count == 1 {
                    self.flags.overflow = bit0 ^ bit15 != 0;
                }
                self.flags.carry = bit0 != 0;
            }
            Op::Ror8 => {
                // Rotate 8 bits of 'dst' right for 'src' times.
                // two arguments
                let mut res = self.read_parameter_value(&op.params.dst) as u8;
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                res = res.rotate_right(count as u32);
                self.write_parameter_u8(&op.params.dst, res);
                let bit6 = (res >> 6) & 1;
                let bit7 = (res >> 7) & 1;
                if count == 1 {
                    self.flags.overflow = bit6 ^ bit7 != 0;
                }
                self.flags.carry = bit7 != 0;
            }
            Op::Ror16 => {
                // Rotate 16 bits of 'dst' right for 'src' times.
                // two arguments
                let mut res = self.read_parameter_value(&op.params.dst) as u16;
                let mut count = self.read_parameter_value(&op.params.src) & 0x1F;
                res = res.rotate_right(count as u32);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, res);
                let bit14 = (res >> 14) & 1;
                let bit15 = (res >> 15) & 1;
                if count == 1 {
                    self.flags.overflow = bit14 ^ bit15 != 0;
                }
                self.flags.carry = bit15 != 0;
            }
            Op::Sahf() => {
                // Store AH into Flags

                // Loads the SF, ZF, AF, PF, and CF flags of the EFLAGS register with values
                // from the corresponding bits in the AH register (bits 7, 6, 4, 2, and 0, respectively).
                let ah = self.get_r8(R8::AH);
                self.flags.carry = ah & 0x1 != 0; // bit 0
                self.flags.parity = ah & 0x4 != 0; // bit 2
                self.flags.auxiliary_carry = ah & 0x10 != 0; // bit 4
                self.flags.zero = ah & 0x40 != 0; // bit 6
                self.flags.sign = ah & 0x80 != 0; // bit 7
            }
            Op::Salc() => {
                // "salc", or "setalc" is a undocumented Intel instruction
                // http://ref.x86asm.net/coder32.html#gen_note_u_SALC_D6
                // http://www.rcollins.org/secrets/opcodes/SALC.html
                // used by dos-software-decoding/demo-256/luminous/luminous.com
                let al = if self.flags.carry {
                    0xFF
                } else {
                    0
                };
                self.set_r8(R8::AL, al);
            }
            Op::Sar8 => {
                // Signed divide* r/m8 by 2, imm8 times.
                // two arguments
                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src) & 0x7;
                if count > 0 {
                    let res = if dst & 0x80 != 0 {
                        let x = 0xFF as usize;
                        dst.rotate_right(count as u32) | x.rotate_left(8 - count as u32)
                    } else {
                        dst.rotate_right(count as u32)
                    };
                    self.write_parameter_u8(&op.params.dst, res as u8);
                    self.flags.carry = (dst as u8 >> (count - 1)) & 0x1 != 0;
                    if count == 1 {
                        self.flags.overflow = false;
                    }
                    self.flags.set_sign_u8(res);
                    self.flags.set_zero_u8(res);
                    self.flags.set_parity(res);
                }
            }
            Op::Sar16 => {
                // Signed divide* r/m8 by 2, imm8 times.
                // two arguments
                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src) & 0xF;
                if count > 0 {
                    let res = if dst & 0x8000 != 0 {
                        let x = 0xFFFF as usize;
                        dst.rotate_right(count as u32) | x.rotate_left(16 - count as u32)
                    } else {
                        dst.rotate_right(count as u32)
                    };
                    self.write_parameter_u16(op.segment_prefix, &op.params.dst, res as u16);
                    self.flags.carry = (dst as u16 >> (count - 1)) & 0x1 != 0;
                    if count == 1 {
                        self.flags.overflow = false;
                    }
                    self.flags.set_sign_u16(res);
                    self.flags.set_zero_u16(res);
                    self.flags.set_parity(res);
                }
            }
            Op::Sbb8() => {
                // Integer Subtraction with Borrow
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let cf = if self.flags.carry { 1 } else { 0 };
                let res = (Wrapping(dst) - (Wrapping(src) + Wrapping(cf))).0;

                // The OF, SF, ZF, AF, PF, and CF flags are set according to the result.
                self.flags.set_overflow_sub_u8(res, src, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
                self.flags.set_carry_u8(res);

                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::Sbb16() => {
                // Integer Subtraction with Borrow

                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let cf = if self.flags.carry { 1 } else { 0 };
                let res = (Wrapping(dst) - (Wrapping(src) + Wrapping(cf))).0;

                // The OF, SF, ZF, AF, PF, and CF flags are set according to the result.
                self.flags.set_overflow_sub_u16(res, src, dst);
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
                self.flags.set_carry_u16(res);

                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            Op::Setc => {
                // setc: Set byte if carry (CF=1).
                // setb (alias): Set byte if below (CF=1).
                let val = if self.flags.carry {
                    1
                } else {
                    0
                };
                self.write_parameter_u8(&op.params.dst, val);
            }
            Op::Setnz => {
                // setnz: Set byte if not zero (ZF=0).
                // setne (alias): Set byte if not equal (ZF=0).
                let val = if !self.flags.zero {
                    1
                } else {
                    0
                };
                self.write_parameter_u8(&op.params.dst, val);
            }
            Op::Shl8 => {
                // Multiply `dst` by 2, `src` times.
                // two arguments    (alias: sal)
                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                if count > 0 {
                    let res = dst.wrapping_shl(count as u32);
                    self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
                    self.flags.carry = (res & 0x80) != 0;
                    if count == 1 {
                        self.flags.overflow = self.flags.carry_val() ^ ((res & 0x80) >> 7) != 0;
                    }
                    self.flags.set_sign_u8(res);
                    self.flags.set_zero_u8(res);
                    self.flags.set_parity(res);
                }
            }
            Op::Shl16 => {
                // Multiply `dst` by 2, `src` times.
                // two arguments    (alias: sal)
                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                if count > 0 {
                    let res = dst.wrapping_shl(count as u32);
                    self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
                    self.flags.carry = (res & 0x8000) != 0;
                    if count == 1 {
                        self.flags.overflow = self.flags.carry_val() ^ ((res & 0x8000) >> 15) != 0;
                    }
                    self.flags.set_sign_u16(res);
                    self.flags.set_zero_u16(res);
                    self.flags.set_parity(res);
                }
            }
            Op::Shld() => {
                // Double Precision Shift Left
                // 3 arguments
                println!("XXX impl {}", op);
            }
            Op::Shr8 => {
                // Unsigned divide r/m8 by 2, `src` times.
                // two arguments
                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                if count > 0 {
                    let res = dst.wrapping_shr(count as u32);
                    self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
                    self.flags.carry = (dst.wrapping_shr((count - 1) as u32) & 0x1) != 0;
                    self.flags.overflow = dst & 0x80 != 0;
                    self.flags.set_sign_u8(res);
                    self.flags.set_zero_u8(res);
                    self.flags.set_parity(res);
                }
            }
            Op::Shr16 => {
                // two arguments
                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src) & 0x1F;
                if count > 0 {
                    let res = dst.wrapping_shr(count as u32);
                    self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
                    self.flags.carry = (dst.wrapping_shr((count - 1) as u32) & 0x1) != 0;
                    self.flags.overflow = dst & 0x8000 != 0;
                    self.flags.set_sign_u16(res);
                    self.flags.set_zero_u16(res);
                    self.flags.set_parity(res);
                }
            }
            Op::Shrd() => {
                // Double Precision Shift Right
                // 3 arguments

                let dst = self.read_parameter_value(&op.params.dst);
                let count = self.read_parameter_value(&op.params.src2);
                if count == 0 {
                    return;
                }
                let src = self.read_parameter_value(&op.params.src);

                // Shift `dst` to right `count` places while shifting bits from `src` in from the left
                let res = (src & count_to_bitmask(count) as usize) << (16-count) | (dst >> count);

                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);

                if count >= 1 {
                    // XXX carry if count is >= 1

                    // If the count is 1 or greater, the CF flag is filled with the last bit shifted out
                    // of the destination operand

                    self.flags.carry = (dst & 1) != 0; // XXX this would be the first bit.. which is wrong
                }

                // SF, ZF, and PF flags are set according to the value of the result.
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_parity(res);

                if count == 1 {
                    // XXX overflow if count == 1
                    // For a 1-bit shift, the OF flag is set if a sign change occurred; otherwise, it is cleared.
                    // For shifts greater than 1 bit, the OF flag is undefined. 
                }

                // If a shift occurs, the AF flag is undefined. If the count is greater than the operand size,
                // the flags are undefined.
            }
            Op::Stc() => {
                // Set Carry Flag
                self.flags.carry = true;
            }
            Op::Std() => {
                // Set Direction Flag
                self.flags.direction = true;
            }
            Op::Sti() => {
                // Set Interrupt Flag
                self.flags.interrupt = true;
            }
            Op::Stosb() => {
                // no parameters
                // store AL at ES:(E)DI
                // The ES segment cannot be overridden with a segment override prefix.
                let al = self.get_r8(R8::AL);
                let es = self.get_sr(&SR::ES);
                let di = self.get_r16(&R16::DI);
                self.mmu.write_u8(es, di, al);
                let di = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::DI)) + Wrapping(1)).0
                } else {
                    (Wrapping(self.get_r16(&R16::DI)) - Wrapping(1)).0
                };
                self.set_r16(&R16::DI, di);
            }
            Op::Stosw() => {
                // no parameters
                // store AX at address ES:(E)DI
                // The ES segment cannot be overridden with a segment override prefix.
                let ax = self.get_r16(&R16::AX);
                let es = self.get_sr(&SR::ES);
                let di = self.get_r16(&R16::DI);
                self.mmu.write_u16(es, di, ax);
                let di = if !self.flags.direction {
                    (Wrapping(self.get_r16(&R16::DI)) + Wrapping(2)).0
                } else {
                    (Wrapping(self.get_r16(&R16::DI)) - Wrapping(2)).0
                };
                self.set_r16(&R16::DI, di);
            }
            Op::Sub8() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) - Wrapping(src)).0;

                // The OF, SF, ZF, AF, PF, and CF flags are set according to the result.
                self.flags.set_overflow_sub_u8(res, src, dst);
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
                self.flags.set_carry_u8(res);

                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::Sub16() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = (Wrapping(dst) - Wrapping(src)).0;

                // The OF, SF, ZF, AF, PF, and CF flags are set according to the result.
                self.flags.set_overflow_sub_u16(res, src, dst);
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_auxiliary(res, src, dst);
                self.flags.set_parity(res);
                self.flags.set_carry_u16(res);

                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            Op::Test8() => {
                // two parameters
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst & src;
                // set SF, ZF, PF according to result.
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_parity(res);
            }
            Op::Test16() => {
                // two parameters
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst & src;
                // set SF, ZF, PF according to result.
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_parity(res);
            }
            Op::Xchg8() => {
                // two parameters (registers)
                let mut src = self.read_parameter_value(&op.params.src);
                let mut dst = self.read_parameter_value(&op.params.dst);
                mem::swap(&mut src, &mut dst);
                self.write_parameter_u8(&op.params.dst, dst as u8);
                self.write_parameter_u8(&op.params.src, src as u8);
            }
            Op::Xchg16() => {
                // two parameters (registers)
                let mut src = self.read_parameter_value(&op.params.src);
                let mut dst = self.read_parameter_value(&op.params.dst);
                mem::swap(&mut src, &mut dst);
                self.write_parameter_u16(op.segment_prefix, &op.params.dst, dst as u16);
                self.write_parameter_u16(op.segment_prefix, &op.params.src, src as u16);
            }
            Op::Xlatb() => {
                // no parameters
                // Set AL to memory byte DS:[(E)BX + unsigned AL].
                // The DS segment may be overridden with a segment override prefix.
                let al = self.mmu.read_u8(self.segment(op.segment_prefix), self.get_r16(&R16::BX) + u16::from(self.get_r8(R8::AL)));
                self.set_r8(R8::AL, al);
            }
            Op::Xor8() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst ^ src;

                // The OF and CF flags are cleared; the SF, ZF,
                // and PF flags are set according to the result.
                self.flags.overflow = false;
                self.flags.carry = false;
                self.flags.set_sign_u8(res);
                self.flags.set_zero_u8(res);
                self.flags.set_parity(res);

                self.write_parameter_u8(&op.params.dst, (res & 0xFF) as u8);
            }
            Op::Xor16() => {
                // two parameters (dst=reg)
                let src = self.read_parameter_value(&op.params.src);
                let dst = self.read_parameter_value(&op.params.dst);
                let res = dst ^ src;

                // The OF and CF flags are cleared; the SF, ZF,
                // and PF flags are set according to the result.
                self.flags.overflow = false;
                self.flags.carry = false;
                self.flags.set_sign_u16(res);
                self.flags.set_zero_u16(res);
                self.flags.set_parity(res);

                self.write_parameter_u16(op.segment_prefix, &op.params.dst, (res & 0xFFFF) as u16);
            }
            _ => {
                println!("execute error: unhandled '{}' at {:04X}:{:04X} (flat {:06X})",
                         op,
                         self.get_sr(&SR::CS),
                         self.ip,
                         self.get_address());
            }
        }

        match op.repeat {
            RepeatMode::Rep => {
                let cx = (Wrapping(self.get_r16(&R16::CX)) - Wrapping(1)).0;
                self.set_r16(&R16::CX, cx);
                if cx != 0 {
                    self.ip = start_ip;
                }
            }
            RepeatMode::Repe => {
                let cx = (Wrapping(self.get_r16(&R16::CX)) - Wrapping(1)).0;
                self.set_r16(&R16::CX, cx);
                if cx != 0 && self.flags.zero {
                    self.ip = start_ip;
                }
            }
            RepeatMode::Repne => {
                let cx = (Wrapping(self.get_r16(&R16::CX)) - Wrapping(1)).0;
                self.set_r16(&R16::CX, cx);
                if cx != 0 && !self.flags.zero {
                    self.ip = start_ip;
                }
            }
            RepeatMode::None => {}
        }

        if op.lock {
            // TODO implement lock
            // println!("XXX FIXME: instruction has LOCK prefix: {}", op);
        }
    }

    fn exception(&mut self, which: Exception, error: usize) {
        /*
        #define CPU_INT_SOFTWARE    0x1
        #define CPU_INT_EXCEPTION   0x2
        #define CPU_INT_HAS_ERROR   0x4
        #define CPU_INT_NOIOPLCHECK 0x8
        */
        println!("Exception {:?}, error {}", which, error);

        // CPU_Interrupt(which,CPU_INT_EXCEPTION | ((which>=8) ? CPU_INT_HAS_ERROR : 0),reg_eip);
    }

    fn cmp16(&mut self, dst: usize, src: usize) {
        let res = (Wrapping(dst) - Wrapping(src)).0;

        // The CF, OF, SF, ZF, AF, and PF flags are set according to the result.
        self.flags.set_carry_u16(res);
        self.flags.set_overflow_sub_u16(res, src, dst);
        self.flags.set_sign_u16(res);
        self.flags.set_zero_u16(res);
        self.flags.set_auxiliary(res, src, dst);
        self.flags.set_parity(res);
    }

    fn push16(&mut self, data: u16) {
        let sp = (Wrapping(self.get_r16(&R16::SP)) - Wrapping(2)).0;
        self.set_r16(&R16::SP, sp);
        let ss = self.get_sr(&SR::SS);
        self.mmu.write_u16(ss, sp, data);
    }

    fn pop16(&mut self) -> u16 {
        let data = self.mmu.read_u16(self.get_sr(&SR::SS), self.get_r16(&R16::SP));
        let sp = (Wrapping(self.get_r16(&R16::SP)) + Wrapping(2)).0;
        self.set_r16(&R16::SP, sp);
        data
    }

    // returns the absoute address of CS:IP
    pub fn get_address(&self) -> usize {
        self.mmu.s_translate(self.get_sr(&SR::CS), self.ip)
    }

    fn read_u8(&mut self) -> u8 {
        let b = self.mmu.read_u8(self.get_sr(&SR::CS), self.ip);
        self.ip += 1;
        b
    }

    fn read_u16(&mut self) -> u16 {
        let lo = self.read_u8();
        let hi = self.read_u8();
        u16::from(hi) << 8 | u16::from(lo)
    }

    fn read_s8(&mut self) -> i8 {
        self.read_u8() as i8
    }

    fn read_s16(&mut self) -> i16 {
        self.read_u16() as i16
    }

    fn read_rel8(&mut self) -> u16 {
        let val = self.read_u8() as i8;
        (self.ip as i16 + i16::from(val)) as u16
    }

    fn read_rel16(&mut self) -> u16 {
        let val = self.read_u16() as i16;
        (self.ip as i16 + val) as u16
    }

    // returns the address of pointer, used by LEA, LDS, LES
    fn read_parameter_address(&mut self, p: &Parameter) -> usize {
        match *p {
            Parameter::Ptr16Amode(_, ref amode) => self.amode16(amode) as usize,
            Parameter::Ptr16AmodeS8(_, ref amode, imm) => (Wrapping(self.amode16(amode) as usize) + Wrapping(imm as usize)).0,
            Parameter::Ptr16AmodeS16(_, ref amode, imm) => (Wrapping(self.amode16(amode) as usize) + Wrapping(imm as usize)).0,
            Parameter::Ptr16(_, imm) => imm as usize,
            _ => {
                println!("read_parameter_address error: unhandled parameter: {:?} at {:06X}",
                         p,
                         self.get_address());
                0
            }
        }
    }

    fn read_parameter_value(&mut self, p: &Parameter) -> usize {
        match p {
            &Parameter::Imm8(imm) => imm as usize,
            &Parameter::Imm16(imm) => imm as usize,
            &Parameter::ImmS8(imm) => imm as usize,
            &Parameter::Ptr8(seg, imm) => {
                self.mmu.read_u8(self.segment(seg), imm) as usize
            }
            &Parameter::Ptr16(seg, imm) => {
                self.mmu.read_u16(self.segment(seg), imm) as usize
            }
            &Parameter::Ptr8Amode(seg, ref amode) => {
                let seg = self.segment(seg);
                let offset = self.amode16(amode);
                self.mmu.read_u8(seg, offset) as usize
            }
            &Parameter::Ptr8AmodeS8(seg, ref amode, imm) => {
                let offset = (Wrapping(self.amode16(amode)) + Wrapping(imm as u16)).0;
                let seg = self.segment(seg);
                self.mmu.read_u8(seg, offset) as usize
            }
            &Parameter::Ptr8AmodeS16(seg, ref amode, imm) => {
                let offset = (Wrapping(self.amode16(amode)) + Wrapping(imm as u16)).0;
                let seg = self.segment(seg);
                self.mmu.read_u8(seg, offset) as usize
            }
            &Parameter::Ptr16Amode(seg, ref amode) => {
                let seg = self.segment(seg);
                let offset = self.amode16(amode);
                self.mmu.read_u16(seg, offset) as usize
            }
            &Parameter::Ptr16AmodeS8(seg, ref amode, imm) => {
                let offset = (Wrapping(self.amode16(amode)) + Wrapping(imm as u16)).0;
                let seg = self.segment(seg);
                self.mmu.read_u16(seg, offset) as usize
            }
            &Parameter::Ptr16AmodeS16(seg, ref amode, imm) => {
                let offset = (Wrapping(self.amode16(amode)) + Wrapping(imm as u16)).0;
                let seg = self.segment(seg);
                self.mmu.read_u16(seg, offset) as usize
            }
            &Parameter::Reg8(ref r) => {
                let r = *r as usize;
                let lor = r & 3;
                if r & 4 == 0 {
                    self.r16[lor].lo_u8() as usize
                } else {
                    self.r16[lor].hi_u8() as usize
                }
            }
            &Parameter::Reg16(ref r) => {
                self.get_r16(r) as usize
            }
            &Parameter::SReg16(ref sr) => {
                self.get_sr(sr) as usize
            },
            _ => {
                println!("read_parameter_value error: unhandled parameter: {:?} at {:06X}",
                         p,
                         self.get_address());
                0
            }
        }
    }

    fn write_parameter_u8(&mut self, p: &Parameter, data: u8) {
        match *p {
            Parameter::Reg8(r) => {
                let r = r as usize;
                let lor = r & 3;
                if r & 4 == 0 {
                    self.r16[lor].set_lo(data);
                } else {
                    self.r16[lor].set_hi(data);
                }
            }
            Parameter::Ptr8(seg, imm) => {
                let seg = self.segment(seg);
                self.mmu.write_u8(seg, imm, data);
            }
            Parameter::Ptr8Amode(seg, ref amode) => {
                let seg = self.segment(seg);
                let offset = self.amode16(amode);
                self.mmu.write_u8(seg, offset, data);
            }
            Parameter::Ptr8AmodeS8(seg, ref amode, imm) => {
                let seg = self.segment(seg);
                let offset = Wrapping(self.amode16(amode)) + Wrapping(imm as u16);
                self.mmu.write_u8(seg, offset.0, data);
            }
            Parameter::Ptr8AmodeS16(seg, ref amode, imm) => {
                let seg = self.segment(seg);
                let offset = Wrapping(self.amode16(amode)) + Wrapping(imm as u16);
                self.mmu.write_u8(seg, offset.0, data);
            }
            _ => {
                println!("write_parameter_u8 unhandled type {:?} at {:06X}",
                         p,
                         self.get_address());
            }
        }
    }

    fn write_parameter_u16(&mut self, segment: Segment, p: &Parameter, data: u16) {
        match p {
            &Parameter::Reg16(ref r) => {
                self.set_r16(r, data);
            }
            &Parameter::SReg16(ref sr) => {
                self.set_sr(sr, data);
            }
            &Parameter::Imm16(imm) => {
                let seg = self.segment(segment);
                self.mmu.write_u16(seg, imm, data);
            }
            &Parameter::Ptr16(seg, imm) => {
                let seg = self.segment(seg);
                self.mmu.write_u16(seg, imm, data);
            }
            &Parameter::Ptr16Amode(seg, ref amode) => {
                let seg = self.segment(seg);
                let offset = self.amode16(amode);
                self.mmu.write_u16(seg, offset, data);
            }
            &Parameter::Ptr16AmodeS8(seg, ref amode, imm) => {
                let seg = self.segment(seg);
                let offset = Wrapping(self.amode16(amode)) + Wrapping(imm as u16);
                self.mmu.write_u16(seg, offset.0, data);
            }
            &Parameter::Ptr16AmodeS16(seg, ref amode, imm) => {
                let seg = self.segment(seg);
                let offset = Wrapping(self.amode16(amode)) + Wrapping(imm as u16);
                self.mmu.write_u16(seg, offset.0, data);
            }
            _ => {
                println!("write_u16_param unhandled type {:?} at {:06X}",
                         p,
                         self.get_address());
            }
        }
    }

    fn segment(&self, seg: Segment) -> u16 {
        match seg {
            Segment::Default |
            Segment::DS => self.get_sr(&SR::DS),
            Segment::CS => self.get_sr(&SR::CS),
            Segment::ES => self.get_sr(&SR::ES),
            Segment::FS => self.get_sr(&SR::FS),
            Segment::GS => self.get_sr(&SR::GS),
            Segment::SS => self.get_sr(&SR::SS),
        }
    }

    fn amode16(&self, amode: &AMode) -> u16 {
        match amode {
            &AMode::BXSI => (Wrapping(self.get_r16(&R16::BX)) + Wrapping(self.get_r16(&R16::SI))).0,
            &AMode::BXDI => (Wrapping(self.get_r16(&R16::BX)) + Wrapping(self.get_r16(&R16::DI))).0,
            &AMode::BPSI => (Wrapping(self.get_r16(&R16::BP)) + Wrapping(self.get_r16(&R16::SI))).0,
            &AMode::BPDI => (Wrapping(self.get_r16(&R16::BP)) + Wrapping(self.get_r16(&R16::DI))).0,
            &AMode::SI => self.get_r16(&R16::SI),
            &AMode::DI => self.get_r16(&R16::DI),
            &AMode::BP => self.get_r16(&R16::BP),
            &AMode::BX => self.get_r16(&R16::BX),
        }
    }

    // used by aaa, aas
    fn adjb(&mut self, param1: i8, param2: i8) {
        if self.flags.auxiliary_carry || (self.get_r8(R8::AL) & 0xf) > 9 {
            let al = self.get_r8(R8::AL);
            let ah = self.get_r8(R8::AH);
            self.set_r8(R8::AL, (u16::from(al) + param1 as u16) as u8);
            self.set_r8(R8::AH, (u16::from(ah) + param2 as u16) as u8);
            self.flags.auxiliary_carry = true;
            self.flags.carry = true;
        } else {
            self.flags.auxiliary_carry = false;
            self.flags.carry = false;
        }
        let al = self.get_r8(R8::AL);
        self.set_r8(R8::AL, al & 0x0F);
    }

    // used by daa, das
    fn adj4(&mut self, param1: i8, param2: i8) {
        let old_al = self.get_r8(R8::AL);
        let old_cf = self.flags.carry;
        self.flags.carry = false;

        if (old_al & 0x0F) > 9 || self.flags.auxiliary_carry {
            let tmp = u16::from(old_al) + param1 as u16;
            self.set_r8(R8::AL, tmp as u8);
            self.flags.carry = tmp & 0x100 != 0;
            self.flags.auxiliary_carry = true;
        } else {
            self.flags.auxiliary_carry = false;
        }

        if old_al > 0x99 || old_cf {
            self.set_r8(R8::AL, (u16::from(old_al) + param2 as u16) as u8);
            self.flags.carry = true;
        }
    }

    // write byte to I/O port
    fn out_u8(&mut self, dst: u16, data: u8) {
        match dst {
            0x03C7 => self.gpu.set_pel_address(data), // XXX unsure if understood correctly
            0x03C8 => self.gpu.set_pel_address(data),
            0x03C9 => self.gpu.set_pel_data(data),
            _ => {
                println!("ERROR: unhandled out_u8 to port {:04X}, data {:02X}", dst, data);
            }
        }
    }

    // write word to I/O port
    fn out_u16(&mut self, dst: u16, data: u16) {
        match dst {
            0x03C4 => {
                // XXX
                /*
                03C4  -W  EGA	TS index register
                        bit7-3 : reserved (VGA only)
                        bit2-0 : current TS index
                03C4  RW  VGA	sequencer register index (see #P0670)
                */
            }
            /*
            0x03C5 => {
                03C5  -W  EGA	TS data register
                03C5  RW  VGA	sequencer register data
            }
            PORT 03D4-03D5 - COLOR VIDEO - CRT CONTROL REGISTERS
            */
            0x03D4 => {
                // 03D4  rW  CRT (6845) register index   (CGA/MCGA/color EGA/color VGA)
                // selects which register (0-11h) is to be accessed through 03D5
                // this port is r/w on some VGA, e.g. ET4000
                //        bit 7-6 =0: (VGA) reserved
                //        bit 5   =0: (VGA) reserved for testage
                //        bit 4-0   : selects which register is to be accessed through 03D5
            }  
            /*
                03D5  -W  CRT (6845) data register   (CGA/MCGA/color EGA/color VGA) (see #P0708)
                    selected by PORT 03D4h. registers 0C-0F may be read
                    (see also PORT 03B5h)
                    MCGA, native EGA and VGA use very different defaults from those
                    mentioned for the other adapters; for additional notes and
                    registers 00h-0Fh and EGA/VGA registers 10h-18h and ET4000
                    registers 32h-37h see PORT 03B5h (see #P0654)
                    registers 10h-11h on CGA, EGA, VGA and 12h-14h on EGA, VGA are
                    conflictive with MCGA (see #P0710)
            */
             _ => {
                println!("XXX unhandled out_u16 to {:04X}, data {:02X}", dst, data);
            }
        }
    }

    // read byte from I/O port
    fn in_port(&mut self, port: u16) -> u8 {
        /*
        println!("in_port: read from {:04X} at {:06X}",
                 port,
                 self.get_offset());
        */
        match port {
            // PORT 0000-001F - DMA 1 - FIRST DIRECT MEMORY ACCESS CONTROLLER (8237)
            0x0002 => {
                // DMA channel 1	current address		byte  0, then byte 1
                println!("XXX fixme in_port read DMA channel 1 current address");
                0
            }
            0x0040 => self.pit.counter0.read_next_part(),
            0x0041 => self.pit.counter1.read_next_part(),
            0x0042 => self.pit.counter2.read_next_part(),
            0x0060 => {
                // PS/2 Controller (keyboard & mice) data port
                // http://wiki.osdev.org/%228042%22_PS/2_Controller
                0 // XXX
            },
            0x03DA => self.gpu.read_cga_status_register(),
            _ => {
                println!("in_port: unhandled in8 {:04X} at {:06X}",
                         port,
                         self.get_address());
                0
            }
        }
    }

    // execute interrupt
    fn int(&mut self, int: u8) {
        match int {
            0x03 => {
                // debugger interrupt
                // http://www.ctyme.com/intr/int-03.htm
                println!("INT 3 - debugger interrupt. AX={:04X}", self.get_r16(&R16::AX));
                self.fatal_error = true; // stops execution
            }
            0x10 => interrupt::int10::handle(self),
            0x16 => interrupt::int16::handle(self),
            0x1A => interrupt::int1a::handle(self),
            0x20 => {
                // DOS 1+ - TERMINATE PROGRAM
                // NOTE: Windows overloads INT 20
                println!("INT 20 - Terminating program");
                self.fatal_error = true; // stops execution
            }
            0x21 => interrupt::int21::handle(self),
            0x33 => interrupt::int33::handle(self),
            _ => {
                println!("int error: unknown interrupt {:02X}, AX={:04X}, BX={:04X}",
                         int,
                         self.get_r16(&R16::AX),
                         self.get_r16(&R16::BX));
            }
        }
    }
}

fn count_to_bitmask(v: usize) -> usize {
    match v {
        0  => 0,
        1  => 0b1,
        2  => 0b11,
        3  => 0b111,
        4  => 0b1111,
        5  => 0b1_1111,
        6  => 0b11_1111,
        7  => 0b111_1111,
        8  => 0b1111_1111,
        9  => 0b1_1111_1111,
        10 => 0b11_1111_1111,
        11 => 0b111_1111_1111,
        12 => 0b1111_1111_1111,
        13 => 0b1_1111_1111_1111,
        14 => 0b11_1111_1111_1111,
        15 => 0b111_1111_1111_1111,
        _ => panic!("unhandled {}", v)
    }
}
