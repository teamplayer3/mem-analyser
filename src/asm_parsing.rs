use regex::Regex;
use std::{fs::File, io::BufRead, num::ParseIntError, ops::Range, path::Path, time::Instant};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AsmError {
    #[error("could not open asm file")]
    FailedOpeningAsmFile(std::io::Error),
    #[error("error while parsing line in file")]
    LineParseError { line: usize, source: std::io::Error },
    #[error("failed parsing addr {0}")]
    AddrParseError(String, ParseIntError),
}

#[derive(Debug, Clone)]
pub enum Instruction {
    Any(String),
    Branch { dest: String },
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub range: Range<u32>,
    pub instructions: Vec<(u32, Instruction)>,
}

#[derive(Debug)]
pub struct AsmFile {
    functions: Vec<Function>,
}

impl AsmFile {
    pub fn from_file(path: &Path) -> Result<Self, AsmError> {
        let file = load_file(path)?;
        parse_asm_file(&file)
    }

    pub fn get_function_based_on_addr(&self, addr: &u32) -> Option<Function> {
        self.functions
            .iter()
            .find(|s| s.range.contains(addr))
            .map(|f| f.to_owned())
    }

    pub fn get_subfunctions_of_function(&self, function: &str) -> Option<Vec<Function>> {
        let mut functions = Vec::<Function>::new();
        let function = self.functions.iter().find(|f| f.name.eq(function));
        let function = match function {
            None => return None,
            Some(f) => f,
        };
        for (addr, instr) in function.instructions.iter() {
            match instr {
                Instruction::Branch { dest } => {
                    if let None = functions.iter().find(|f| f.name.eq(dest.as_str())) {
                        functions.push(
                            self.functions
                                .iter()
                                .find(|f| f.name.eq(dest.as_str()))
                                .unwrap()
                                .to_owned(),
                        );
                    }
                }
                _ => (),
            }
        }

        Some(functions)
    }
}

fn load_file(path: &Path) -> Result<File, AsmError> {
    std::fs::File::open(path).map_err(|e| AsmError::FailedOpeningAsmFile(e))
}

struct FunctionHeader {
    name: String,
    start_addr: u32,
    instructions: Vec<(u32, Instruction)>,
}

impl FunctionHeader {
    fn new(name: String, start_addr: u32) -> Self {
        Self {
            name,
            start_addr,
            instructions: Vec::new(),
        }
    }
    fn complete(self) -> Function {
        Function {
            range: self.start_addr..self.instructions.last().unwrap().0 + 1,
            name: self.name,
            instructions: self.instructions,
        }
    }
}

fn parse_asm_file(file: &File) -> Result<AsmFile, AsmError> {
    let mut asm_file = AsmFile {
        functions: Vec::new(),
    };
    let buf_reader = std::io::BufReader::new(file).lines();

    let function_heading = Regex::new(r"(?P<addr>[\d\w]+) <(?P<func_name>[\s\S]+)>:").unwrap();
    let instruction_line = Regex::new(r" (?P<addr>[\d\w]+):	(?P<instr_line>[\s\S]*)").unwrap();
    let instruction_bl = Regex::new(r"[\s\S]+	bl[\s\S]+<(?P<func_name>[\s\S]+)>").unwrap();

    let mut actual_function: Option<FunctionHeader> = None;
    for (index, l) in buf_reader.enumerate() {
        let line = l.map_err(|e| AsmError::LineParseError {
            line: index,
            source: e,
        })?;

        if let Some(captures) = function_heading.captures(&line) {
            let function_name = &captures["func_name"];
            let function_addr = &captures["addr"];
            let function_addr = u32::from_str_radix(function_addr, 16)
                .map_err(|e| AsmError::AddrParseError(String::from(function_addr), e))?;
            let function = FunctionHeader::new(String::from(function_name), function_addr);

            let old = actual_function.replace(function);
            if let Some(f) = old {
                asm_file.functions.push(f.complete());
            }
        } else if let Some(captures) = instruction_line.captures(&line) {
            let instr_line = &captures["instr_line"];
            let instr_addr = &captures["addr"];
            let instr_addr = u32::from_str_radix(instr_addr, 16)
                .map_err(|e| AsmError::AddrParseError(String::from(instr_addr), e))?;
            let instruction = if let Some(captures) = instruction_bl.captures(instr_line) {
                let dest_func = &captures["func_name"];
                Instruction::Branch {
                    dest: String::from(dest_func),
                }
            } else {
                Instruction::Any(String::from(instr_line))
            };
            if let Some(ref mut func) = actual_function {
                func.instructions.push((instr_addr, instruction))
            }
        }
    }

    if let Some(func) = actual_function.take() {
        asm_file.functions.push(func.complete())
    }

    Ok(asm_file)
}
