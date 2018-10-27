use std::time::{SystemTime, UNIX_EPOCH};
use std::time::Instant;

use hardware::Hardware;
use cpu::{CPU, R};

// time related interrupts
pub fn handle(cpu: &mut CPU, hw: &mut Hardware) {
    match cpu.get_r8(R::AH) {
        0x00 => {
            // TIME - GET SYSTEM TIME
            // Return:
            // CX:DX = number of clock ticks since midnight
            // AL = midnight flag, nonzero if midnight passed since time last read
            if cpu.deterministic {
                cpu.set_r16(R::CX, 0);
                cpu.set_r16(R::DX, 0);
                cpu.set_r8(R::AL, 0);
            } else {
                // println!("INT 1A GET TIME: get number of clock ticks since midnight, ticks {}",  hw.pit.timer0.count);
                let cx = (hw.pit.timer0.count >> 16) as u16;
                let dx = (hw.pit.timer0.count & 0xFFFF) as u16;
                cpu.set_r16(R::CX, cx);
                cpu.set_r16(R::DX, dx);
                cpu.set_r8(R::AL, 0); // TODO implement midnight flag
            }
        }
        0x01 => {
            // TIME - SET SYSTEM TIME
            // CX:DX = number of clock ticks since midnight
            let cx = cpu.get_r16(R::CX);
            let dx = cpu.get_r16(R::DX);
            let ticks = (cx as u32) << 16 | dx as u32;

            hw.pit.timer0.count = ticks;
            // println!("SET SYSTEM TIME to {}", ticks);
        }
        _ => {
            println!("int1a (time) error: unknown ah={:02X}, ax={:04X}",
                     cpu.get_r8(R::AH),
                     cpu.get_r16(R::AX));
        }
    }
}
