use std::io::{self, Read, Write};
use std::fs::File;
use std::process::Command;
use std::str;

use tempdir::TempDir;

use cpu::{Encoder, Instruction};

pub fn ndisasm_bytes(bytes: &[u8]) -> Result<String, io::Error> {
    let tmp_dir = TempDir::new("ndisasm")?;
    let file_path = tmp_dir.path().join("binary.bin");
    let file_str = file_path.to_str().unwrap();
    let mut tmp_file = File::create(&file_path)?;

    tmp_file.write_all(bytes)?;

    let output = Command::new("ndisasm")
        .args(&["-b", "16", file_str])
        .output()
        .expect("failed to execute process");

    drop(tmp_file);
    tmp_dir.close()?;

    let s = str::from_utf8(&output.stdout).unwrap().trim();

    // parse syntax "00000000  CD21              int 0x21", return third column
    let mut col = 0;
    let mut spacing = false;
    let mut res = String::new();
    for c in s.chars() {
        if c == ' ' {
            if !spacing && col < 2 {
                col += 1;
                spacing = true;
            }
        } else {
            spacing = false;
        }
        if col == 2 {
            res.push(c);
        }
    }

    Ok(res.trim().to_owned())
}

/// encodes an instruction and then disasms the resulting byte sequence with external ndisasm command
fn ndisasm_instruction(op: &Instruction) -> Result<String, io::Error> {
    let encoder = Encoder::new();
    if let Ok(data) = encoder.encode(op) {
        return ndisasm_bytes(&data);
    } else {
        panic!("invalid byte sequence");
    }
}