use hardware::Hardware;
use cpu::CPU;
use cpu::register::{R16, SR};

// mouse related interrupts
pub fn handle(cpu: &mut CPU, hw: &mut Hardware) {
    match cpu.get_r16(&R16::AX) {
        0x0003 => {
            // MS MOUSE v1.0+ - RETURN POSITION AND BUTTON STATUS
            // Return:
            // BX = button status (see #03168)
            // CX = column
            // DX = row
            // Note: In text modes, all coordinates are specified as multiples of the cell size, typically 8x8 pixels 
            println!("XXX impl MOUSE - RETURN POSITION AND BUTTON STATUS");
        }
        _ => {
            println!("int33 error: unknown ax={:04X}, ip={:04X}:{:04X}",
                     cpu.get_r16(&R16::AX),
                     cpu.get_sr(&SR::CS),
                     cpu.ip);
        }
    }
}
