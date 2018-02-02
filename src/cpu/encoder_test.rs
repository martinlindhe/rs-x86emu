use cpu::Encoder;
use cpu::CPU;
use cpu::RepeatMode;
use cpu::Segment;
use cpu::{Parameter, ParameterPair};
use cpu::instruction::{Instruction, InstructionInfo, Op};
use cpu::register::SR;
use memory::mmu::MMU;


#[test]
fn can_encode_instr() {

    let mmu = MMU::new();
    let mut cpu = CPU::new(mmu);
    let code: Vec<u8> = vec![
        0xCD, 0x21, // int 0x21
    ];
    cpu.load_com(&code);

    let cs = cpu.get_sr(&SR::CS);
    let ops = cpu.decoder.decode_to_block(cs, 0x100, 1);
    assert_eq!(vec!(
        InstructionInfo{
            bytes: vec!(0xCD,0x21),
            segment: cs as usize,
            offset: 0x100,
            text: "Int      0x21".to_owned(),
            instruction: Instruction{
                command: Op::Int(),
                segment_prefix: Segment::Default,
                length: 2,
                lock: false,
                repeat: RepeatMode::None,
                params: ParameterPair {
                    dst: Parameter::Imm8(0x21),
                    src: Parameter::None(),
                    src2: Parameter::None(),
                }
            }
        }), ops);
}
