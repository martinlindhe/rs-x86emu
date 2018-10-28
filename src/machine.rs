use std::time::{Duration, SystemTime};

use bincode::deserialize;

use cpu::{CPU, Op, Invalid, R, RegisterSnapshot, Segment, OperandSize};
use gpu::GPU;
use hardware::Hardware;
use hex::hex_bytes;
use memory::{MMU, MemoryAddress};
use ndisasm::{ndisasm_bytes, ndisasm_first_instr};

/// prints each instruction as they are executed
const DEBUG_EXEC: bool = false;

#[derive(Deserialize, Debug)]
struct ExeHeader {
    signature: u16,             // 0x5A4D == "MZ"
    bytes_in_last_block: u16,   // padding info for exact data size
    blocks_in_file: u16,        // data size in 512-byte blocks
    num_relocs: u16,            // number of relocation items
    header_paragraphs: u16,     // header size in 16-byte paragraphs
    min_extra_paragraphs: u16,
    max_extra_paragraphs: u16,
    ss: u16,
    sp: u16,
    checksum: u16,
    ip: u16,
    cs: u16,
    reloc_table_offset: u16,
    overlay_number: u16,
}

#[derive(Deserialize, Debug)]
struct ExeReloc {
    offset: u16,
    segment: u16,
}

struct Exe {
    header: ExeHeader,
    relocs: Vec<ExeReloc>,
}

pub struct Machine {
    pub hw: Hardware,
    pub cpu: CPU,

    /// base offset where rom was loaded
    pub rom_base: MemoryAddress,

    /// length of loaded rom in bytes (used by disassembler)
    pub rom_length: usize,

    last_update: SystemTime,
}

impl Machine {
    pub fn default() -> Self {
        Machine {
            cpu: CPU::default(),
            hw: Hardware::default(),
            rom_base: MemoryAddress::default_real(),
            rom_length: 0,
            last_update: SystemTime::now(),
        }
    }

    pub fn deterministic() -> Self {
        let mut cpu = CPU::default();
        cpu.deterministic = true;

        Machine {
            cpu: cpu,
            hw: Hardware::deterministic(),
            rom_base: MemoryAddress::default_real(),
            rom_length: 0,
            last_update: SystemTime::now(),
        }
    }

    /// reset the CPU and memory
    pub fn hard_reset(&mut self) {
        self.cpu = CPU::default();
    }

    pub fn load_executable(&mut self, data: &[u8]) {
        if data[0] == b'M' && data[1] == b'Z' {
            self.load_exe(data);
        } else {
            self.load_com(data);
        }
    }

    fn load_exe(&mut self, data: &[u8]) {
        let hdr: ExeHeader = deserialize(data).unwrap();
        println!("load_exe header: {:?}", hdr);

        let reloc_from = hdr.reloc_table_offset as usize;
        let reloc_to = hdr.reloc_table_offset as usize + (hdr.num_relocs as usize * 4);
        println!("load_exe read relocs from {:04X}-{:04X}", reloc_from, reloc_to);

        // let relocs: Vec<ExeReloc> = deserialize(&data[reloc_from..reloc_to]).unwrap();  // XXX crashes
        let relocs: ExeReloc = deserialize(&data[reloc_from..reloc_to]).unwrap(); // XXX only reads first reloc
        println!("XXX relocs: {:?}", relocs);

        let code_offset = hdr.header_paragraphs as usize * 16;
        let mut code_end = hdr.blocks_in_file as usize * 512;
        if hdr.bytes_in_last_block > 0 {
            code_end -= 512 - hdr.bytes_in_last_block as usize;
        }
        println!("load exe code from {:04X}:{:04X}", code_offset, code_end);

        self.load_com(&data[code_offset..code_end]);
        self.cpu.set_r16(R::SP, hdr.sp); // confirmed
        self.cpu.set_r16(R::SS, hdr.ss); // XXX dosbox = 0923
        
        // at program start in dosbox-x:
        // BP = 091C (dustbox ok)
        // SP = 1000 (from hdr, dustbox ok)
        // CS = 0920
        // DS = 0910
        // ES = 0910
        // SS = 0923
    }

    /// load .com program into CS:0100 and set IP to program start
    fn load_com(&mut self, data: &[u8]) {

        // CS,DS,ES,SS = PSP segment
        let psp_segment = 0x085F; // is what dosbox used
        self.cpu.set_r16(R::CS, psp_segment);
        self.cpu.set_r16(R::DS, psp_segment);
        self.cpu.set_r16(R::ES, psp_segment);
        self.cpu.set_r16(R::SS, psp_segment);

        // offset of last word available in first 64k segment
        self.cpu.set_r16(R::SP, 0xFFFE);
        self.cpu.set_r16(R::BP, 0x091C); // is what dosbox used

        // This is what dosbox initializes the registers to
        // at program load
        self.cpu.set_r16(R::CX, 0x00FF);
        self.cpu.set_r16(R::DX, psp_segment);
        self.cpu.set_r16(R::SI, 0x0100); // XXX 0 on .exe load
        self.cpu.set_r16(R::DI, 0xFFFE); // XXX 0x1000 on .exe

        self.cpu.regs.ip = 0x0100;
        self.rom_base = self.cpu.get_memory_address();
        self.rom_length = data.len();

        let cs = self.cpu.get_r16(R::CS);
        self.hw.mmu.write(cs, self.cpu.regs.ip, data);

        self.cpu.mark_stack(&mut self.hw.mmu);
    }

    /// returns a copy of register values at a given time
    pub fn register_snapshot(&self) -> RegisterSnapshot {
        self.cpu.regs.clone()
    }

    /// executes enough instructions that can run for 1 video frame
    pub fn execute_frame(&mut self) {
        let fps = 60;
        let cycles = self.cpu.clock_hz / fps;
        // println!("will execute {} cycles", cycles);

        loop {
            self.execute_instruction();
            if self.cpu.fatal_error {
                break;
            }
            if self.cpu.cycle_count > cycles {
                self.cpu.cycle_count = 0;
                break;
            }
        }
    }

    /// executes n instructions of the cpu
    pub fn execute_instructions(&mut self, count: usize) {
        for _ in 0..count {
            self.execute_instruction();
            if self.cpu.fatal_error {
                break;
            }
        }
    }

    /// returns first line of disassembly
    fn external_disasm_of_bytes(&self, cs: u16, ip: u16) -> String {
        let bytes = self.hw.mmu.read(cs, ip, 16);
        ndisasm_first_instr(&bytes).unwrap().to_owned()
    }

    pub fn execute_instruction(&mut self) {
        // XXX this is wrong!!!  time based timer increment is source of random output in morales.com
        if self.last_update.elapsed().unwrap() > Duration::from_millis(55) {
            self.last_update = SystemTime::now();
            self.hw.pit.update(&mut self.hw.mmu);
        }

        let cs = self.cpu.get_r16(R::CS);
        let ip = self.cpu.regs.ip;
        if cs == 0xF000 {
            // we are in interrupt vector code, execute high-level interrupt.
            // the default interrupt vector table has a IRET
            self.cpu.handle_interrupt(&mut self.hw, ip as u8);
        }

        let op = self.cpu.decoder.get_instruction(&mut self.hw.mmu, cs, ip);

        match op.command {
            Op::Uninitialized => {
                self.cpu.fatal_error = true;
                println!("[{:04X}:{:04X}] ERROR: uninitialized op. {} instructions executed",
                         cs, ip, self.cpu.instruction_count);
            }
            Op::Invalid(bytes, reason) => {
                let hex = hex_bytes(&bytes);
                self.cpu.fatal_error = true;
                match reason {
                    Invalid::Op => {
                        println!("[{:04X}:{:04X}] {} ERROR: unhandled opcode", cs, ip, hex);
                        println!("ndisasm: {}", self.external_disasm_of_bytes(cs, ip));
                    }
                    Invalid::FPUOp => {
                        println!("[{:04X}:{:04X}] {} ERROR: unhandled FPU opcode", cs, ip, hex);
                        println!("ndisasm: {}", self.external_disasm_of_bytes(cs, ip));
                    }
                    Invalid::Reg(reg) => {
                        println!("[{:04X}:{:04X}] {} ERROR: unhandled reg value {:02X}", cs, ip, hex, reg);
                        println!("ndisasm: {}", self.external_disasm_of_bytes(cs, ip));
                    }
                }
                println!("{} Instructions executed", self.cpu.instruction_count);
            }
            _ => {
                if DEBUG_EXEC {
                    println!("[{:04X}:{:04X}] {}", cs, ip, op);
                }
                self.cpu.execute(&mut self.hw, &op);
            },
        }

        // XXX need instruction timing to do this properly
        if self.cpu.cycle_count % 100 == 0 {
            self.hw.gpu.progress_scanline();
        }
    }
}
