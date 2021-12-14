use std::{sync::MutexGuard, time::Duration};

use probe_rs::{
    config::{MemoryRegion, NvmRegion, RamRegion},
    flashing::DownloadOptions,
    Core, Session,
};

use crate::asm_parsing::AsmFile;

pub struct CPU<'a> {
    session: MutexGuard<'a, Session>,
}

impl<'a> CPU<'a> {
    const DURATION: Duration = Duration::from_secs(5);
    pub fn new(session: MutexGuard<'a, Session>) -> Self {
        Self { session }
    }

    pub fn reset_and_halt(&mut self) -> std::result::Result<(), probe_rs::Error> {
        let mut core = self.session.core(0)?;
        core.reset_and_halt(Self::DURATION)?;

        Ok(())
    }

    pub fn halt(&mut self) -> std::result::Result<(), probe_rs::Error> {
        let mut core = self.session.core(0)?;
        core.halt(Self::DURATION)?;

        Ok(())
    }

    pub fn run(&mut self) -> std::result::Result<(), probe_rs::Error> {
        let mut core = self.session.core(0)?;
        core.run()?;

        Ok(())
    }

    pub fn step(&mut self) -> std::result::Result<(), probe_rs::Error> {
        let mut core = self.session.core(0)?;
        core.step()?;

        Ok(())
    }

    pub fn run_to_point(&mut self, addr: u32) -> std::result::Result<(), probe_rs::Error> {
        let mut core = self.session.core(0)?;
        core.set_hw_breakpoint(addr)?;
        core.run()?;
        core.wait_for_core_halted(Self::DURATION)
            .expect("Breakpoint not reached before timeout");
        core.clear_hw_breakpoint(addr)?;

        Ok(())
    }

    pub fn halt_while<T, F: FnMut(&mut Core) -> std::result::Result<T, probe_rs::Error>>(
        &mut self,
        mut func: F,
    ) -> std::result::Result<T, probe_rs::Error> {
        self.halt()?;
        let res = {
            let mut core = self.session.core(0)?;
            func(&mut core)?
        };
        self.run()?;

        Ok(res)
    }

    pub fn access_only_in_halt_mode<
        T,
        F: FnMut(&mut Core) -> std::result::Result<T, probe_rs::Error>,
    >(
        &mut self,
        mut func: F,
    ) -> std::result::Result<T, probe_rs::Error> {
        let prev_state_halt = {
            let mut core = self.session.core(0)?;
            core.core_halted()?
        };

        if !prev_state_halt {
            self.halt()?;
        }
        let res = {
            let mut core = self.session.core(0)?;
            func(&mut core)?
        };
        if !prev_state_halt {
            self.run()?;
        }

        Ok(res)
    }

    pub fn access_core<T, F: FnMut(&mut Core) -> std::result::Result<T, probe_rs::Error>>(
        &mut self,
        mut func: F,
    ) -> std::result::Result<T, probe_rs::Error> {
        let mut core = self.session.core(0)?;
        func(&mut core)
    }

    pub fn flash_region(&mut self) -> std::result::Result<NvmRegion, probe_rs::Error> {
        let flash_region = self
            .session
            .target()
            .memory_map
            .iter()
            .filter(|m| match m {
                MemoryRegion::Nvm(m) => m.is_boot_memory,
                _ => false,
            })
            .collect::<Vec<_>>();

        let region = match flash_region[0] {
            MemoryRegion::Nvm(m) => m.clone(),
            _ => unreachable!(),
        };

        Ok(region)
    }

    pub fn ram_region(&mut self) -> std::result::Result<RamRegion, probe_rs::Error> {
        let ram_region = self
            .session
            .target()
            .memory_map
            .iter()
            .filter(|m| match m {
                MemoryRegion::Ram(_) => true,
                _ => false,
            })
            .collect::<Vec<_>>();

        let region = match ram_region[0] {
            MemoryRegion::Ram(m) => m.clone(),
            _ => unreachable!(),
        };

        Ok(region)
    }

    pub fn flash(
        &mut self,
        mut file: std::fs::File,
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let target = self.session.target();
        let mut loader = target.flash_loader();
        loader.load_elf_data(&mut file)?;
        let options = DownloadOptions::default();
        loader.commit(&mut *self.session, options)?;
        self.reset_and_halt()?;

        Ok(())
    }

    fn step_over_act_func(
        &mut self,
        asm_file: &AsmFile,
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut core = self.session.core(0)?;
        if !core.core_halted()? {
            core.halt(Self::DURATION)?;
        }

        let instr_ptr = core.read_core_reg(core.registers().program_counter())?;
        let function = asm_file.get_function_based_on_addr(&instr_ptr).unwrap();

        let start = std::time::Instant::now();
        while std::time::Instant::now() - start < Duration::from_secs(1) {
            // core.write_core_reg(, value)
            core.step()?;
            let instr_ptr = core.read_core_reg(core.registers().program_counter())?;
            if !function.range.contains(&instr_ptr) {
                break;
            }
        }

        Ok(())
    }
}
