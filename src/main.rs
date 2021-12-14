#![feature(drain_filter)]

use std::{
    fs::File,
    io::{ErrorKind, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use clap::{App, Arg};
use object::{Object, ObjectSection};
use probe_rs::{MemoryInterface, Probe};

use text_io::read;

use crate::mem_monitoring::{calculate_used_ram, cpu_monitor, RamSnapshot, RamSnapshotRecorder};

mod asm_parsing;
mod cpu;
mod mem_monitoring;
mod registers;

type DynError<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn read_bin_file<P>(path: P) -> DynError<Vec<u8>>
where
    P: AsRef<Path>,
{
    Ok(std::fs::read(path)?)
}

struct ConnectionHandler {
    streams: Arc<Mutex<Vec<TcpStream>>>,
    server: JoinHandle<()>,
}

impl ConnectionHandler {
    fn new() -> Self {
        let streams = Arc::new(Mutex::new(Vec::new()));
        let streams_tmp = streams.to_owned();
        let server = std::thread::spawn(move || {
            let streams = streams_tmp;
            let tcp = TcpListener::bind("127.0.0.10:80").expect("could'nt bind to address");
            while let Ok((stream, _)) = tcp.accept() {
                streams.lock().unwrap().push(stream);
            }
        });

        Self { streams, server }
    }

    fn distribute(&mut self, json_str: &str) -> std::io::Result<()> {
        let mut streams = self.streams.lock().unwrap();
        let mut to_close_connections = Vec::new();
        for stream in streams.as_mut_slice() {
            match (*stream).write_all(json_str.as_bytes()) {
                Err(e) => match e.kind() {
                    ErrorKind::ConnectionAborted => {
                        to_close_connections.push(stream.peer_addr().unwrap());
                        Ok(())
                    }
                    _ => Err(e),
                },
                _ => Ok(()),
            }?;
        }

        for to_close in to_close_connections {
            let mut streams = self.streams.lock().unwrap();
            streams.drain_filter(|t| t.peer_addr().unwrap().eq(&to_close));
        }

        Ok(())
    }
}

enum AnalyseMode {
    Looping,
    SingleShot,
    Stepping,
    LoopMeasure,
}

fn main() -> DynError<()> {
    let asm_file = asm_parsing::AsmFile::from_file(Path::new("./tmp/.asm_arduino"))?;

    // println!(
    //     "{:?}",
    //     asm_file
    //         .get_subfunctions_of_function(&"loop")
    //         .unwrap()
    //         .iter()
    //         .map(|f| f.name.to_owned())
    //         .collect::<Vec<_>>()
    // );
    // return Ok(());

    let matches = App::new("Stack Analyser")
        .version("0.1.0")
        .author("Alexander H. <alex.teamplayer@gmail.com>")
        .arg(
            Arg::with_name("firmware_path")
                .short("f")
                .value_name("FIRMWARE_PATH")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("language")
                .possible_values(&["rust", "cpp"])
                .value_name("LANGUAGE")
                .short("l")
                .default_value("rust")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("no_flash")
                .takes_value(false)
                .short("n")
                .value_name("NO_FLASH"),
        )
        .arg(
            Arg::with_name("mode")
                .value_name("MODE")
                .short("m")
                .long("mode")
                .takes_value(true)
                .possible_values(&["stepping", "looping", "single-shot", "loop-measure"])
                .default_value("looping"),
        )
        .arg(
            Arg::with_name("start_addr")
                .value_name("START_ADDR")
                .long("start-addr")
                .short("s")
                .takes_value(true)
                .help("Sets start address of measuring if in stepping mode."),
        )
        .get_matches();
    let elf_path = matches.value_of("firmware_path").unwrap();
    let is_cpp = match matches.value_of("language").unwrap() {
        "cpp" => true,
        _ => false,
    };
    let should_flash = match matches.value_of("no_flash") {
        Some(_) => false,
        None => true,
    };
    let analyse_mode = match matches.value_of("mode").unwrap() {
        "stepping" => AnalyseMode::Stepping,
        "looping" => AnalyseMode::Looping,
        "single-shot" => AnalyseMode::SingleShot,
        "loop-measure" => AnalyseMode::LoopMeasure,
        _ => unreachable!(),
    };

    let start_instr_addr: Option<u32> = matches
        .value_of("start_addr")
        .and_then(|s| Some(u32::from_str_radix(s, 16).unwrap()));

    let file = read_bin_file(elf_path)?;
    let obj_file = object::File::parse(file.as_slice())?;

    let stack_start_ptr = if let Some(vec_section) = obj_file.section_by_name(if !is_cpp {
        ".vector_table"
    } else {
        ".isr_vector"
    }) {
        let data = vec_section.data()?;
        u32::from_le_bytes([data[0], data[1], data[2], data[3]])
    } else {
        panic!(".vector_table section required in obj file");
    };

    // let mut connection_handler = ConnectionHandler::new();

    // let heap_section = obj_file
    //     .section_by_name(".heap")
    //     .expect("no .heap section in obj file");

    // let defmt_table = defmt_decoder::Table::parse(file.as_slice())?;
    // let locations = defmt_table.unwrap().get_locations(file.as_slice())?;
    // println!("defmt_locations = {:?}", locations);

    let probes = Probe::list_all();
    let probe = probes[0].open()?;
    let session = Arc::new(Mutex::new(probe.attach("STM32G431RBTx")?));

    // let mut rtt = Rtt::attach(session.to_owned())?;
    // println!("{:?}", rtt.up_channels());

    let mut session = session.lock().unwrap();
    let mut cpu = cpu::CPU::new(session);
    cpu.halt()?;

    // let mem_map = session.target().memory_map;

    let ram_region = cpu.ram_region()?;
    let flash_region = cpu.flash_region()?;

    cpu.access_core(|core| {
        for reg in ram_region.clone().range {
            core.write_word_8(reg, 0x55)?;
        }

        Ok(())
    })?;

    if should_flash {
        let file = File::open(elf_path)?;
        println!("start flashing");
        cpu.flash(file)?;
        println!("flashed");
    } else {
        cpu.reset_and_halt()?;
    }

    let analyse_interval = Duration::from_millis(100);
    let mut recorder = RamSnapshotRecorder::new(
        (ram_region.range.end - stack_start_ptr) as usize,
        analyse_interval.to_owned(),
    );

    println!("start measuring");

    let now = std::time::Instant::now();

    match analyse_mode {
        AnalyseMode::Looping => {
            if start_instr_addr.is_some() {
                cpu.run_to_point(*start_instr_addr.as_ref().unwrap())?;
            }
            loop {
                let ram = calculate_used_ram(stack_start_ptr, &mut cpu, &asm_file)?;
                recorder.record(ram);

                std::thread::sleep(analyse_interval);
                if std::time::Instant::now() - now > Duration::from_secs(60) {
                    break;
                }
            }
        }
        AnalyseMode::Stepping => {
            if start_instr_addr.is_some() {
                cpu.run_to_point(*start_instr_addr.as_ref().unwrap())?;
            }

            loop {
                cpu.step()?;
                let ram = calculate_used_ram(stack_start_ptr, &mut cpu, &asm_file)?;
                recorder.record(ram);

                let line: String = read!("{}\n");
                if line.starts_with("c") {
                    break;
                } else {
                    continue;
                }
            }
        }
        AnalyseMode::SingleShot => {
            if start_instr_addr.is_none() {
                panic!("start_addr is needed")
            }

            let ram = calculate_used_ram(stack_start_ptr, &mut cpu, &asm_file)?;
            println!("start stack usage: {}", ram);

            cpu.run_to_point(start_instr_addr.unwrap())?;

            let ram = calculate_used_ram(stack_start_ptr, &mut cpu, &asm_file)?;
            println!("at point stack usage: {}", ram);
        }
        AnalyseMode::LoopMeasure => {
            if start_instr_addr.is_none() {
                panic!("start_addr is needed")
            }

            let mut cpu_records = Vec::new();

            cpu.run_to_point(start_instr_addr.unwrap())?;
            cpu.run()?;
            loop {
                let cpu_snapshot = cpu_monitor(stack_start_ptr, &mut cpu)?;
                cpu_records.push(cpu_snapshot);
                std::thread::sleep(analyse_interval);
                if std::time::Instant::now() - now > Duration::from_secs(60) {
                    break;
                }
            }

            println!(
                "{:?}",
                cpu_records
                    .iter()
                    .map(|r| r.stack_ptr_off)
                    .collect::<Vec<_>>()
                    .as_slice()
            );
        }
    }

    // {
    //     let mut core = session.core(0)?;
    //     const START: u32 = 0x20004B58;
    //     const END: u32 = 0x20008000;
    //     const REGION_SIZE: usize = END as usize - START as usize;
    //     let mut buffer = Vec::with_capacity(REGION_SIZE);
    //     unsafe { buffer.set_len(REGION_SIZE) }
    //     core.read(START, buffer.as_mut())?;

    //     for i in buffer.as_slice() {
    //         println!("data {}", *i);
    //     }
    // }

    let statistics = recorder.calculate_statistics();
    println!("{:?}", statistics);

    let record_file_content = serde_json::to_string(&recorder)?;
    let mut record_file = File::create("record.json")?;
    record_file.write(record_file_content.as_bytes())?;

    Ok(())
}
