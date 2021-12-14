use std::{fmt::Display, ops::Range, sync::MutexGuard, time::Duration};

use probe_rs::{MemoryInterface, Session};
use serde::Serialize;
use serde_hex::{SerHex, StrictPfx};

use crate::{asm_parsing::AsmFile, cpu, DynError};

struct UsedRange {
    start: u32,
}

impl UsedRange {
    fn new(start: u32) -> Self {
        Self { start }
    }

    fn complete(self, end: u32) -> Range<u32> {
        end..self.start
    }
}

#[derive(Debug, Clone, Eq, Serialize)]
pub struct RamSnapshot {
    used_bytes: u32,
    stack_ptr_offset: u32,
    ranges: Vec<Range<u32>>,
    #[serde(with = "SerHex::<StrictPfx>")]
    instr_ptr: u32,
    function: String,
}

impl PartialEq for RamSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.used_bytes == other.used_bytes
            && self.stack_ptr_offset == other.stack_ptr_offset
            && self.ranges == other.ranges
    }
}

impl PartialOrd for RamSnapshot {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.used_bytes.partial_cmp(&other.used_bytes) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        match self.stack_ptr_offset.partial_cmp(&other.stack_ptr_offset) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }

        Some(std::cmp::Ordering::Equal)
    }
}

impl Ord for RamSnapshot {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl Display for RamSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RamSnapshot {{ instruction: 0x{:08x}, used_bytes: {}, stack_ptr_offset: {}, ranges: {:?}, function: {} }}", &self.instr_ptr, &self.used_bytes, &self.stack_ptr_offset, &self.ranges, &self.function)
    }
}

#[derive(Debug)]
pub struct RamStatistics {
    median_stack_ptr_off: u32,
    max_stack_ptr_off: u32,
    max_mem_usage: u32,
    stack_ptr_course: Vec<u32>,
    mem_usage_course: Vec<u32>,
}

#[derive(Serialize)]
pub struct RamSnapshotRecorder {
    analyse_interval: Duration,
    static_ram_size: usize,
    snapshot_variants: Vec<RamSnapshot>,
    records: Vec<usize>,
}

impl RamSnapshotRecorder {
    pub fn new(static_ram_size: usize, analyse_interval: Duration) -> Self {
        Self {
            analyse_interval,
            static_ram_size,
            snapshot_variants: Vec::new(),
            records: Vec::new(),
        }
    }

    pub fn record(&mut self, snapshot: RamSnapshot) {
        let sp = self.snapshot_variants.iter().position(|r| r.eq(&snapshot));
        match sp {
            Some(index) => self.records.push(index),
            None => {
                self.snapshot_variants.push(snapshot);
                self.records.push(self.snapshot_variants.len() - 1);
            }
        }
    }

    pub fn calculate_statistics(&self) -> RamStatistics {
        let mut stack_ptrs_off = self
            .records
            .iter()
            .map(|r| self.snapshot_variants[*r].stack_ptr_offset)
            .collect::<Vec<_>>();

        let stack_ptr_course = stack_ptrs_off.to_owned();

        stack_ptrs_off.sort_unstable_by(|x: &u32, y: &u32| x.partial_cmp(y).unwrap());
        let median_stack_ptr_off = percentile_of_sorted(stack_ptrs_off.as_slice(), 50.0);

        let max_stack_ptr_off = *stack_ptrs_off.last().unwrap();

        let mut max_mem_usage = self
            .records
            .iter()
            .map(|r| self.snapshot_variants[*r].used_bytes)
            .collect::<Vec<_>>();
        let mem_usage_course = max_mem_usage.to_owned();
        max_mem_usage.sort_unstable_by(|x: &u32, y: &u32| x.partial_cmp(y).unwrap());
        let max_mem_usage = *max_mem_usage.last().unwrap();

        RamStatistics {
            median_stack_ptr_off,
            max_stack_ptr_off,
            max_mem_usage,
            stack_ptr_course,
            mem_usage_course,
        }
    }

    pub fn get_records(&mut self) -> RamSnapshotRecords {
        RamSnapshotRecords {
            pos: 0,
            records: self,
        }
    }
}

// Helper function: extract a value representing the `pct` percentile of a sorted sample-set, using
// linear interpolation. If samples are not sorted, return nonsensical value.
fn percentile_of_sorted(sorted_samples: &[u32], pct: f32) -> u32 {
    assert!(!sorted_samples.is_empty());
    if sorted_samples.len() == 1 {
        return sorted_samples[0];
    }
    assert!(0.0 <= pct);
    assert!(pct <= 100.0);
    if pct == 100.0 {
        return sorted_samples[sorted_samples.len() - 1];
    }
    let length = (sorted_samples.len() - 1) as f32;
    let rank = (pct / 100.0) * length;
    let lrank = rank.floor();
    let d = rank - lrank;
    let n = lrank as usize;
    let lo = sorted_samples[n];
    let hi = sorted_samples[n + 1];
    lo + (hi - lo) * d as u32
}

pub struct RamSnapshotRecords<'a> {
    pos: usize,
    records: &'a RamSnapshotRecorder,
}

impl Iterator for RamSnapshotRecords<'_> {
    type Item = RamSnapshot;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos == self.records.records.len() {
            return None;
        }
        let snap_index = self.records.records[self.pos];
        Some(self.records.snapshot_variants[snap_index].clone())
    }
}

fn print_ranges(ranges: Vec<Range<u32>>) {
    for r in ranges {
        println!(
            "[{:#08x}..{:#08x}] len: {}",
            r.start,
            r.end,
            r.end - r.start
        );
    }
}

struct HeapSnapshot {
    used_bytes: u32,
}

fn monitor_heap(
    session: &mut MutexGuard<Session>,
    heap_start: u32,
    heap_size: u32,
) -> DynError<HeapSnapshot> {
    let mut core = session.core(0)?;

    const BYTE_PATTERN: u8 = 0x55;

    Ok(HeapSnapshot { used_bytes: 0 })
}

pub fn calculate_used_ram(
    stack_ptr: u32,
    cpu: &mut cpu::CPU,
    asm_file: &AsmFile,
) -> DynError<RamSnapshot> {
    const BYTE_PATTERN: u8 = 0x55;

    let mut used_bytes = 0;
    let mut address = stack_ptr - 1;
    const TEST_OFFSET: usize = 128;
    const OFFSET_BETWEEN_RANGES: usize = 4 * 5;
    let mut offset_mem = Vec::<u8>::with_capacity(OFFSET_BETWEEN_RANGES);
    let mut in_offset_flag = false;
    let mut test_offset = TEST_OFFSET;
    let mut act_range: Option<UsedRange> = None;

    let res = cpu.access_only_in_halt_mode(move |core| {
        let mut ranges = Vec::<Range<u32>>::new();
        while let Ok(m) = core.read_word_8(address) {
            let byte_not_overridden = m == BYTE_PATTERN;

            if in_offset_flag {
                offset_mem.push(m);
                if TEST_OFFSET - test_offset > OFFSET_BETWEEN_RANGES {
                    match act_range {
                        Some(_) => {
                            let mut not_used_in_mem = 0;
                            for mb in offset_mem.iter().rev() {
                                if *mb == 0x55 {
                                    not_used_in_mem += 1;
                                }
                            }
                            let end_range = address + not_used_in_mem;
                            let range = act_range.take().unwrap().complete(end_range);
                            ranges.push(range);
                            offset_mem.clear();
                        }
                        None => (),
                    }
                }
                if !byte_not_overridden {
                    in_offset_flag = false;
                    used_bytes += 1;
                    match act_range {
                        None => {
                            let _ = act_range.insert(UsedRange::new(address));
                        }
                        _ => (),
                    };
                    test_offset = TEST_OFFSET;
                } else if test_offset == 0 {
                    break;
                } else {
                    test_offset -= 1;
                }
            }

            if byte_not_overridden {
                in_offset_flag = true;
            } else {
                used_bytes += 1;
                match act_range {
                    None => {
                        let _ = act_range.insert(UsedRange::new(address));
                    }
                    _ => (),
                };
            }
            address -= 1;
        }

        // core.halt(Duration::from_millis(10))?;
        let act_stack_ptr = core.read_core_reg(core.registers().stack_pointer())?;
        let instr_ptr = core.read_core_reg(core.registers().program_counter())?;

        // core.run()?;
        // println!(
        //     "act_stack_ptr: {:#08x}, stack_start_ptr: {:#08x}, used_bytes: {}",
        //     act_stack_ptr, stack_ptr, used_bytes
        // );
        // println!("act {}", act_stack_ptr);
        let stack_ptr_offset = stack_ptr - act_stack_ptr;

        Ok(RamSnapshot {
            ranges,
            stack_ptr_offset,
            used_bytes,
            function: asm_file
                .get_function_based_on_addr(&instr_ptr)
                .unwrap()
                .name,
            instr_ptr,
        })
    })?;

    Ok(res)
}

#[derive(Debug)]
pub struct CPUSnapshot {
    pub instr_ptr: u32,
    pub stack_ptr_off: u32,
}

pub fn cpu_monitor(stack_ptr: u32, cpu: &mut cpu::CPU) -> DynError<CPUSnapshot> {
    let res = cpu.access_only_in_halt_mode(|core| {
        let act_stack_ptr = core.read_core_reg(core.registers().stack_pointer())?;
        let instr_ptr = core.read_core_reg(core.registers().program_counter())?;
        let stack_ptr_off = stack_ptr - act_stack_ptr;

        Ok(CPUSnapshot {
            instr_ptr,
            stack_ptr_off,
        })
    })?;

    Ok(res)
}
