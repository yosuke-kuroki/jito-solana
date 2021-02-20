#![allow(clippy::upper_case_acronyms)]

use crate::BpfError;
use solana_rbpf::ebpf;
use thiserror::Error;

/// Error definitions
#[derive(Debug, Error, PartialEq)]
pub enum VerifierError {
    /// ProgramLengthNotMultiple
    #[error("program length must be a multiple of {} octets", ebpf::INSN_SIZE)]
    ProgramLengthNotMultiple,
    /// ProgramTooLarge
    #[error("program too big, max {}, is {}", ebpf::PROG_MAX_INSNS, .0)]
    ProgramTooLarge(usize),
    /// NoProgram
    #[error("no program set, call prog_set() to load one")]
    NoProgram,
    #[error("division by 0 (insn #{0})")]
    DivisionByZero(usize),
    /// UnsupportedLEBEArgument
    #[error("unsupported argument for LE/BE (insn #{0})")]
    UnsupportedLEBEArgument(usize),
    /// LDDWCannotBeLast
    #[error("LD_DW instruction cannot be last in program")]
    LDDWCannotBeLast,
    /// IncompleteLDDW
    #[error("incomplete LD_DW instruction (insn #{0})")]
    IncompleteLDDW(usize),
    /// InfiniteLoop
    #[error("infinite loop (insn #{0})")]
    InfiniteLoop(usize),
    /// JumpOutOfCode
    #[error("jump out of code to #{0} (insn #{1})")]
    JumpOutOfCode(usize, usize),
    /// JumpToMiddleOfLDDW
    #[error("jump to middle of LD_DW at #{0} (insn #{1})")]
    JumpToMiddleOfLDDW(usize, usize),
    /// InvalidSourceRegister
    #[error("invalid source register (insn #{0})")]
    InvalidSourceRegister(usize),
    /// CannotWriteR10
    #[error("cannot write into register r10 (insn #{0})")]
    CannotWriteR10(usize),
    /// InvalidDestinationRegister
    #[error("invalid destination register (insn #{0})")]
    InvalidDestinationRegister(usize),
    /// UnknownOpCode
    #[error("unknown eBPF opcode {0:#2x} (insn #{1:?})")]
    UnknownOpCode(u8, usize),
    /// Shift with overflow
    #[error("Shift with overflow at instruction {0}")]
    ShiftWithOverflow(usize),
    /// Invalid register specified
    #[error("Invalid register specified at instruction {0}")]
    InvalidRegister(usize),
}

fn adj_insn_ptr(insn_ptr: usize) -> usize {
    insn_ptr + ebpf::ELF_INSN_DUMP_OFFSET
}

fn check_prog_len(prog: &[u8]) -> Result<(), BpfError> {
    if prog.len() % ebpf::INSN_SIZE != 0 {
        return Err(VerifierError::ProgramLengthNotMultiple.into());
    }
    if prog.len() > ebpf::PROG_MAX_SIZE {
        return Err(VerifierError::ProgramTooLarge(prog.len() / ebpf::INSN_SIZE).into());
    }

    if prog.is_empty() {
        return Err(VerifierError::NoProgram.into());
    }
    Ok(())
}

fn check_imm_nonzero(insn: &ebpf::Insn, insn_ptr: usize) -> Result<(), BpfError> {
    if insn.imm == 0 {
        return Err(VerifierError::DivisionByZero(adj_insn_ptr(insn_ptr)).into());
    }
    Ok(())
}

fn check_imm_endian(insn: &ebpf::Insn, insn_ptr: usize) -> Result<(), BpfError> {
    match insn.imm {
        16 | 32 | 64 => Ok(()),
        _ => Err(VerifierError::UnsupportedLEBEArgument(adj_insn_ptr(insn_ptr)).into()),
    }
}

fn check_load_dw(prog: &[u8], insn_ptr: usize) -> Result<(), BpfError> {
    if insn_ptr + 1 >= (prog.len() / ebpf::INSN_SIZE) {
        // Last instruction cannot be LD_DW because there would be no 2nd DW
        return Err(VerifierError::LDDWCannotBeLast.into());
    }
    let next_insn = ebpf::get_insn(prog, insn_ptr + 1);
    if next_insn.opc != 0 {
        return Err(VerifierError::IncompleteLDDW(adj_insn_ptr(insn_ptr)).into());
    }
    Ok(())
}

fn check_jmp_offset(prog: &[u8], insn_ptr: usize) -> Result<(), BpfError> {
    let insn = ebpf::get_insn(prog, insn_ptr);
    // if insn.off == -1 {
    //     return Err(VerifierError::InfiniteLoop(adj_insn_ptr(insn_ptr)).into());
    // }

    let dst_insn_ptr = insn_ptr as isize + 1 + insn.off as isize;
    if dst_insn_ptr < 0 || dst_insn_ptr as usize >= (prog.len() / ebpf::INSN_SIZE) {
        return Err(
            VerifierError::JumpOutOfCode(dst_insn_ptr as usize, adj_insn_ptr(insn_ptr)).into(),
        );
    }
    let dst_insn = ebpf::get_insn(prog, dst_insn_ptr as usize);
    if dst_insn.opc == 0 {
        return Err(VerifierError::JumpToMiddleOfLDDW(
            dst_insn_ptr as usize,
            adj_insn_ptr(insn_ptr),
        )
        .into());
    }
    Ok(())
}

fn check_registers(insn: &ebpf::Insn, store: bool, insn_ptr: usize) -> Result<(), BpfError> {
    if insn.src > 10 {
        return Err(VerifierError::InvalidSourceRegister(adj_insn_ptr(insn_ptr)).into());
    }
    match (insn.dst, store) {
        (0..=9, _) | (10, true) => Ok(()),
        (10, false) => Err(VerifierError::CannotWriteR10(adj_insn_ptr(insn_ptr)).into()),
        (_, _) => Err(VerifierError::InvalidDestinationRegister(adj_insn_ptr(insn_ptr)).into()),
    }
}

/// Check that the imm is a valid shift operand
fn check_imm_shift(insn: &ebpf::Insn, insn_ptr: usize) -> Result<(), VerifierError> {
    if insn.imm < 0 || insn.imm as u64 >= 64 {
        return Err(VerifierError::ShiftWithOverflow(adj_insn_ptr(insn_ptr)));
    }
    Ok(())
}

/// Check that the imm is a valid register number
fn check_imm_register(insn: &ebpf::Insn, insn_ptr: usize) -> Result<(), VerifierError> {
    if insn.imm < 0 || insn.imm > 10 {
        return Err(VerifierError::InvalidRegister(adj_insn_ptr(insn_ptr)));
    }
    Ok(())
}

#[rustfmt::skip]
pub fn check(prog: &[u8]) -> Result<(), BpfError> {
    check_prog_len(prog)?;

    let mut insn_ptr: usize = 0;
    while insn_ptr * ebpf::INSN_SIZE < prog.len() {
        let insn = ebpf::get_insn(prog, insn_ptr);
        let mut store = false;

        match insn.opc {

            // BPF_LD class
            ebpf::LD_ABS_B   => {},
            ebpf::LD_ABS_H   => {},
            ebpf::LD_ABS_W   => {},
            ebpf::LD_ABS_DW  => {},
            ebpf::LD_IND_B   => {},
            ebpf::LD_IND_H   => {},
            ebpf::LD_IND_W   => {},
            ebpf::LD_IND_DW  => {},

            ebpf::LD_DW_IMM  => {
                store = true;
                check_load_dw(prog, insn_ptr)?;
                insn_ptr += 1;
            },

            // BPF_LDX class
            ebpf::LD_B_REG   => {},
            ebpf::LD_H_REG   => {},
            ebpf::LD_W_REG   => {},
            ebpf::LD_DW_REG  => {},

            // BPF_ST class
            ebpf::ST_B_IMM   => store = true,
            ebpf::ST_H_IMM   => store = true,
            ebpf::ST_W_IMM   => store = true,
            ebpf::ST_DW_IMM  => store = true,

            // BPF_STX class
            ebpf::ST_B_REG   => store = true,
            ebpf::ST_H_REG   => store = true,
            ebpf::ST_W_REG   => store = true,
            ebpf::ST_DW_REG  => store = true,

            // BPF_ALU class
            ebpf::ADD32_IMM  => {},
            ebpf::ADD32_REG  => {},
            ebpf::SUB32_IMM  => {},
            ebpf::SUB32_REG  => {},
            ebpf::MUL32_IMM  => {},
            ebpf::MUL32_REG  => {},
            ebpf::DIV32_IMM  => { check_imm_nonzero(&insn, insn_ptr)?; },
            ebpf::DIV32_REG  => {},
            ebpf::OR32_IMM   => {},
            ebpf::OR32_REG   => {},
            ebpf::AND32_IMM  => {},
            ebpf::AND32_REG  => {},
            ebpf::LSH32_IMM  => { check_imm_shift(&insn, insn_ptr)?; },
            ebpf::LSH32_REG  => {},
            ebpf::RSH32_IMM  => { check_imm_shift(&insn, insn_ptr)?; },
            ebpf::RSH32_REG  => {},
            ebpf::NEG32      => {},
            ebpf::MOD32_IMM  => { check_imm_nonzero(&insn, insn_ptr)?; },
            ebpf::MOD32_REG  => {},
            ebpf::XOR32_IMM  => {},
            ebpf::XOR32_REG  => {},
            ebpf::MOV32_IMM  => {},
            ebpf::MOV32_REG  => {},
            ebpf::ARSH32_IMM => { check_imm_shift(&insn, insn_ptr)?; },
            ebpf::ARSH32_REG => {},
            ebpf::LE         => { check_imm_endian(&insn, insn_ptr)?; },
            ebpf::BE         => { check_imm_endian(&insn, insn_ptr)?; },

            // BPF_ALU64 class
            ebpf::ADD64_IMM  => {},
            ebpf::ADD64_REG  => {},
            ebpf::SUB64_IMM  => {},
            ebpf::SUB64_REG  => {},
            ebpf::MUL64_IMM  => { check_imm_nonzero(&insn, insn_ptr)?; },
            ebpf::MUL64_REG  => {},
            ebpf::DIV64_IMM  => { check_imm_nonzero(&insn, insn_ptr)?; },
            ebpf::DIV64_REG  => {},
            ebpf::OR64_IMM   => {},
            ebpf::OR64_REG   => {},
            ebpf::AND64_IMM  => {},
            ebpf::AND64_REG  => {},
            ebpf::LSH64_IMM  => { check_imm_shift(&insn, insn_ptr)?; },
            ebpf::LSH64_REG  => {},
            ebpf::RSH64_IMM  => { check_imm_shift(&insn, insn_ptr)?; },
            ebpf::RSH64_REG  => {},
            ebpf::NEG64      => {},
            ebpf::MOD64_IMM  => { check_imm_nonzero(&insn, insn_ptr)?; },
            ebpf::MOD64_REG  => {},
            ebpf::XOR64_IMM  => {},
            ebpf::XOR64_REG  => {},
            ebpf::MOV64_IMM  => {},
            ebpf::MOV64_REG  => {},
            ebpf::ARSH64_IMM => { check_imm_shift(&insn, insn_ptr)?; },
            ebpf::ARSH64_REG => {},

            // BPF_JMP class
            ebpf::JA         => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JEQ_IMM    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JEQ_REG    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JGT_IMM    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JGT_REG    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JGE_IMM    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JGE_REG    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JLT_IMM    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JLT_REG    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JLE_IMM    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JLE_REG    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSET_IMM   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSET_REG   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JNE_IMM    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JNE_REG    => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSGT_IMM   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSGT_REG   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSGE_IMM   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSGE_REG   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSLT_IMM   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSLT_REG   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSLE_IMM   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::JSLE_REG   => { check_jmp_offset(prog, insn_ptr)?; },
            ebpf::CALL_IMM   => {},
            ebpf::CALL_REG   => { check_imm_register(&insn, insn_ptr)?; },
            ebpf::EXIT       => {},

            _                => {
                return Err(VerifierError::UnknownOpCode(insn.opc, adj_insn_ptr(insn_ptr)).into());
            }
        }

        check_registers(&insn, store, insn_ptr)?;

        insn_ptr += 1;
    }

    // insn_ptr should now be equal to number of instructions.
    if insn_ptr != prog.len() / ebpf::INSN_SIZE {
        return Err(VerifierError::JumpOutOfCode(adj_insn_ptr(insn_ptr), adj_insn_ptr(insn_ptr)).into());
    }

    Ok(())
}
