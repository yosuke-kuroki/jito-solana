#![feature(test)]
#![cfg(feature = "bpf_c")]
extern crate test;

use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use solana_rbpf::EbpfVm;
use solana_sdk::{
    account::Account,
    bpf_loader,
    entrypoint_native::{ComputeMeter, InvokeContext, Logger, ProcessInstruction},
    instruction::{CompiledInstruction, InstructionError},
    message::Message,
    pubkey::Pubkey,
};
use std::{cell::RefCell, rc::Rc};
use std::{env, fs::File, io::Read, mem, path::PathBuf};
use test::Bencher;

/// BPF program file extension
const PLATFORM_FILE_EXTENSION_BPF: &str = "so";
/// Create a BPF program file name
fn create_bpf_path(name: &str) -> PathBuf {
    let mut pathbuf = {
        let current_exe = env::current_exe().unwrap();
        PathBuf::from(current_exe.parent().unwrap().parent().unwrap())
    };
    pathbuf.push("bpf/");
    pathbuf.push(name);
    pathbuf.set_extension(PLATFORM_FILE_EXTENSION_BPF);
    pathbuf
}

fn empty_check(_prog: &[u8]) -> Result<(), solana_bpf_loader_program::BPFError> {
    Ok(())
}

fn load_elf() -> Result<Vec<u8>, std::io::Error> {
    let path = create_bpf_path("bench_alu");
    let mut file = File::open(&path).expect(&format!("Unable to open {:?}", path));
    let mut elf = Vec::new();
    file.read_to_end(&mut elf).unwrap();
    Ok(elf)
}

const ARMSTRONG_LIMIT: u64 = 500;
const ARMSTRONG_EXPECTED: u64 = 5;

#[bench]
fn bench_program_load_elf(bencher: &mut Bencher) {
    let elf = load_elf().unwrap();
    let mut vm = EbpfVm::<solana_bpf_loader_program::BPFError>::new(None).unwrap();
    vm.set_verifier(empty_check).unwrap();

    bencher.iter(|| {
        vm.set_elf(&elf).unwrap();
    });
}

#[bench]
fn bench_program_verify(bencher: &mut Bencher) {
    let elf = load_elf().unwrap();
    let mut vm = EbpfVm::<solana_bpf_loader_program::BPFError>::new(None).unwrap();
    vm.set_verifier(empty_check).unwrap();
    vm.set_elf(&elf).unwrap();

    bencher.iter(|| {
        vm.set_verifier(solana_bpf_loader_program::bpf_verifier::check)
            .unwrap();
    });
}

#[bench]
fn bench_program_alu(bencher: &mut Bencher) {
    let ns_per_s = 1000000000;
    let one_million = 1000000;
    let mut inner_iter = vec![];
    inner_iter
        .write_u64::<LittleEndian>(ARMSTRONG_LIMIT)
        .unwrap();
    inner_iter.write_u64::<LittleEndian>(0).unwrap();
    let mut invoke_context = MockInvokeContext::default();

    let elf = load_elf().unwrap();
    let (mut vm, _) =
        solana_bpf_loader_program::create_vm(&bpf_loader::id(), &elf, &[], &mut invoke_context)
            .unwrap();

    println!("Interpreted:");
    assert_eq!(
        0, /*success*/
        vm.execute_program(&mut inner_iter, &[], &[]).unwrap()
    );
    assert_eq!(ARMSTRONG_LIMIT, LittleEndian::read_u64(&inner_iter));
    assert_eq!(
        ARMSTRONG_EXPECTED,
        LittleEndian::read_u64(&inner_iter[mem::size_of::<u64>()..])
    );

    bencher.iter(|| {
        vm.execute_program(&mut inner_iter, &[], &[]).unwrap();
    });
    let instructions = vm.get_total_instruction_count();
    let summary = bencher.bench(|_bencher| {}).unwrap();
    println!("  {:?} instructions", instructions);
    println!("  {:?} ns/iter median", summary.median as u64);
    assert!(0f64 != summary.median);
    let mips = (instructions * (ns_per_s / summary.median as u64)) / one_million;
    println!("  {:?} MIPS", mips);
    println!("{{ \"type\": \"bench\", \"name\": \"bench_program_alu_interpreted_mips\", \"median\": {:?}, \"deviation\": 0 }}", mips);

    // JIT disabled until address translation support is added
    // println!("JIT to native:");
    // vm.jit_compile().unwrap();
    // unsafe {
    //     assert_eq!(
    //         0, /*success*/
    //         vm.execute_program_jit(&mut inner_iter).unwrap()
    //     );
    // }
    // assert_eq!(ARMSTRONG_LIMIT, LittleEndian::read_u64(&inner_iter));
    // assert_eq!(
    //     ARMSTRONG_EXPECTED,
    //     LittleEndian::read_u64(&inner_iter[mem::size_of::<u64>()..])
    // );

    // bencher.iter(|| unsafe {
    //     vm.execute_program_jit(&mut inner_iter).unwrap();
    // });
    // let summary = bencher.bench(|_bencher| {}).unwrap();
    // println!("  {:?} instructions", instructions);
    // println!("  {:?} ns/iter median", summary.median as u64);
    // assert!(0f64 != summary.median);
    // let mips = (instructions * (ns_per_s / summary.median as u64)) / one_million;
    // println!("  {:?} MIPS", mips);
    // println!("{{ \"type\": \"bench\", \"name\": \"bench_program_alu_jit_to_native_mips\", \"median\": {:?}, \"deviation\": 0 }}", mips);
}

#[derive(Debug, Default)]
pub struct MockInvokeContext {
    key: Pubkey,
    mock_logger: MockLogger,
    mock_compute_meter: MockComputeMeter,
}
impl InvokeContext for MockInvokeContext {
    fn push(&mut self, _key: &Pubkey) -> Result<(), InstructionError> {
        Ok(())
    }
    fn pop(&mut self) {}
    fn verify_and_update(
        &mut self,
        _message: &Message,
        _instruction: &CompiledInstruction,
        _accounts: &[Rc<RefCell<Account>>],
    ) -> Result<(), InstructionError> {
        Ok(())
    }
    fn get_caller(&self) -> Result<&Pubkey, InstructionError> {
        Ok(&self.key)
    }
    fn get_programs(&self) -> &[(Pubkey, ProcessInstruction)] {
        &[]
    }
    fn get_logger(&self) -> Rc<RefCell<dyn Logger>> {
        Rc::new(RefCell::new(self.mock_logger.clone()))
    }
    fn is_cross_program_supported(&self) -> bool {
        true
    }
    fn get_compute_meter(&self) -> Rc<RefCell<dyn ComputeMeter>> {
        Rc::new(RefCell::new(self.mock_compute_meter.clone()))
    }
}
#[derive(Debug, Default, Clone)]
pub struct MockLogger {
    pub log: Rc<RefCell<Vec<String>>>,
}
impl Logger for MockLogger {
    fn log_enabled(&self) -> bool {
        true
    }
    fn log(&mut self, message: &str) {
        self.log.borrow_mut().push(message.to_string());
    }
}
#[derive(Debug, Default, Clone)]
pub struct MockComputeMeter {
    pub remaining: u64,
}
impl ComputeMeter for MockComputeMeter {
    fn consume(&mut self, amount: u64) -> Result<(), InstructionError> {
        self.remaining = self.remaining.saturating_sub(amount);
        if self.remaining == 0 {
            return Err(InstructionError::ComputationalBudgetExceeded);
        }
        Ok(())
    }
    fn get_remaining(&self) -> u64 {
        self.remaining
    }
}
