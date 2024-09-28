use regex::Regex;
use std::env::*;
use std::fs;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::process::exit;

mod levenshtein_hashmap;
mod types;
use levenshtein_hashmap::LevenshteinHashMap;
use types::{DupsFile, Function, Instruction};
// parse .s file to get instructions and function name
fn parse_instructions(input: &str, dir: &str, file: &str) -> Function {
    let mut instructions = Vec::new();
    let mut func_name = "";

    for line in input.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();

        // find the function name
        if parts.len() == 2 {
            if parts[0] == "glabel" {
                func_name = parts[1];
            }
        }

        if parts.len() < 3 {
            continue; // Skip lines that don't have enough parts
        }

        if let Ok(file_addr) = u64::from_str_radix(parts[1], 16) {
            if let Ok(vram_addr) = u64::from_str_radix(parts[2], 16) {
                if let Ok(op) = u32::from_str_radix(parts[3], 16) {
                    // splat's output for the instruction is apparently little-endian
                    let reversed_num = ((op >> 24) & 0xFF)
                        | (((op >> 16) & 0xFF) << 8)
                        | (((op >> 8) & 0xFF) << 16)
                        | ((op & 0xFF) << 24);

                    // if the file address, vram address, and instruction parsed, add it
                    let instruction = Instruction {
                        file_addr,
                        vram_addr,
                        op: reversed_num,
                    };

                    instructions.push(instruction);
                }
            }
        }
    }

    // use the 'op' part of the instruction to find duplicates
    // (bits above the 26th)
    let key: Vec<u8> = instructions
        .iter()
        .map(|num| (num.op >> 26) as u8)
        .collect();

    Function {
        ops: instructions,
        name: func_name.to_string(),
        key: key,
        dir: dir.to_string(),
        file: file.to_string(),
        similarity: 0.0,
        decompiled: false,
    }
}

fn apply_sliding_window(parsed_func: &Function, n: usize, k: usize) -> Vec<Function> {
    let mut functions = Vec::new();
    let instructions = &parsed_func.ops;
    let func_name = &parsed_func.name;

    for i in (0..instructions.len()).step_by(n) {
        if i + k > instructions.len() {
            break; // Stop if there are not enough elements for a full window
        }

        // Create a new function with the current sliding window
        let window_ops = instructions[i..i + k].to_vec();
        let window_key: Vec<u8> = window_ops
            .iter()
            .map(|num| (num.op >> 26) as u8)
            .collect();

        let new_func_name = format!("{}:{}:{}", func_name, i, i + k - 1);

        functions.push(Function {
            ops: window_ops,
            name: new_func_name.clone(),
            key: window_key,
            dir: parsed_func.dir.clone(),
            file: parsed_func.file.clone(),
            similarity: 0.0,
            decompiled: false,
        });
    }

    functions
}

fn process_directory(dir_path: &str, funcs: &mut Vec<Function>) {
    match std::fs::read_dir(dir_path) {
        Ok(entries) => {
            entries.for_each(|entry| {
                if let Ok(entry) = entry {
                    let item_path = entry.path();
                    if item_path.is_file() && item_path.to_string_lossy().ends_with(".s") {
                        println!("checking {:?}", item_path);

                        let mut file = fs::File::open(item_path.clone()).unwrap();
                        let mut buffer = String::new();
                        file.read_to_string(&mut buffer).unwrap();

                        let func =
                            parse_instructions(&buffer, &dir_path, &item_path.to_string_lossy());

                        // jr $ra, nop
                        let is_null = func.ops.len() == 2
                            && func.ops[0].op == 0x03E00008
                            && func.ops[1].op == 0x00000000;
                        if !is_null {
                            funcs.push(func.clone());

                            let windows = apply_sliding_window(&func, 4, 32);
                            windows.into_iter().for_each(|w| {
                                funcs.push(w.clone());
                            });
                        }
                    } else if item_path.is_dir() {
                        process_directory(&item_path.to_string_lossy(), funcs);
                    }
                }
            });
        }
        Err(error) => {
            eprintln!("Unable to read directory: {}", error);
            println!("Directory path: {}", dir_path);
            exit(1);
        }
    }
}

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about,
    long_about = "\n
Finds duplicates in two asm directories and prints them out in order to identify patterns

Usage:

make force_extract

Do a 2-way compare with ordering
cargo run --release -- --dir ../../asm/us/st/nz0/nonmatchings/ --dir ../../asm/us/st/np3/nonmatchings/ --threshold .94

Clustering report for all overlays
cargo run --release -- --threshold .94 --output-file output.txt
"
)]

struct Args {
    /// Levenshtein similarity threshold
    #[arg(short, long)]
    threshold: f64,

    /// Directory to parse asm from (2 required)
    #[arg(short, long)]
    dir: Vec<String>,

    /// File to write output to
    #[arg(short, long)]
    output_file: Option<String>,

    /// Base of source directory
    #[arg(short, long)]
    src_base: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IncludeAsmEntry {
    pub line: String,
    pub path: String,
    pub asm_path: String,
}

fn process_directory_for_include_asm(dir: &str) -> Vec<IncludeAsmEntry> {
    let entries = std::fs::read_dir(dir).expect("Unable to read directory");

    let re = Regex::new("INCLUDE_ASM\\(\"([^\"]*)\", ([^)]*)\\)").unwrap();
    let mut output = Vec::new();

    entries.for_each(|entry| {
        if let Ok(entry) = entry {
            let item_path = entry.path();
            if item_path.is_file() && item_path.to_string_lossy().ends_with(".c") {
                println!("checking {:?}", item_path);

                let file = File::open(item_path.clone()).expect("Unable to open file");
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    let line_str = line.unwrap();

                    if line_str.contains("INCLUDE_ASM") {
                        let (full, [asm_dir, asm_file]) = re.captures(&line_str).unwrap().extract();

                        output.push(IncludeAsmEntry {
                            line: line_str.clone(),
                            path: item_path.to_string_lossy().to_string(),
                            asm_path: format!("../../asm/us/{}/{}.s", asm_dir, asm_file),
                        });
                    }
                }
            } else if item_path.is_dir() {
                process_directory_for_include_asm(&item_path.to_string_lossy());
            }
        }
    });
    output
}

fn get_all_include_asm(dir: &str) -> Vec<IncludeAsmEntry> {
    process_directory_for_include_asm(dir)
}
#[derive(Clone)]
struct SrcAsmPair {
    asm_dir: String,
    src_dir: String,
    overlay_name: String,
    include_asm: Vec<IncludeAsmEntry>,
    path_matcher: String,
}

fn do_dups_report(output_file: Option<String>, threshold: f64) {
    // full dups report
    let mut hash_map = LevenshteinHashMap::new(threshold);

    let mut files = Vec::new();

    let pairs: Vec<SrcAsmPair> = vec![
        SrcAsmPair {
            asm_dir: String::from("../../asm/matchings"),
            src_dir: String::from("../../src/os/"),
            overlay_name: String::from("OS"),
            include_asm: get_all_include_asm("../../src/os/"),
            path_matcher: "/os/".to_string(),
        },
        // SrcAsmPair {
        //     asm_dir: String::from("../../asm/nonmatchings"),
        //     src_dir: String::from("../../src/os/"),
        //     overlay_name: String::from("OS"),
        //     include_asm: get_all_include_asm("../../src/os/"),
        //     path_matcher: "/os/".to_string(),
        // },
    ];

    for pair in pairs.clone() {
        let dir = pair.asm_dir;
        let mut funcs = Vec::new();
        process_directory(&dir, &mut funcs);

        // sort functions by vram address
        funcs.sort_by_key(|function| {
            function
                .ops
                .first()
                .map_or(u64::MAX, |instr| instr.vram_addr)
        });

        files.push(DupsFile {
            name: dir.to_string(),
            funcs: funcs.clone(),
        });
    }

    for file in &files {
        println!("file {}", file.name);
        for func in &file.funcs {
            println!("\t{} {}", func.name, func.ops.len());
        }
    }

    for file in &files {
        for func in &file.funcs {
            hash_map.insert(func.key.clone(), func.clone());
        }
    }

    let mut entries: Vec<(&Vec<u8>, &Vec<Function>)> = hash_map.map.iter().collect();

    // sort by filename
    entries.sort_by(|(_, functions1), (_, functions2)| functions1[0].file.cmp(&functions2[0].file));

    // Then sort by the length of functions in reverse order
    entries.sort_by_key(|(_, functions)| std::cmp::Reverse(functions.len()));

    if let o_file = output_file.unwrap() {
        let mut output_file = File::create(o_file).expect("Unable to create file");
        writeln!(
            output_file,
            "| {:<4} | {:<8} | {:<35} | {:<2} ",
            "%", "Decomp?", "Name", "Asm Path"
        )
        .expect("Error writing to file");

        for (_, functions) in entries {
            if functions.len() > 1 {
                // Write separator to file
                writeln!(output_file, "-------------------------------------------------------------------------------")
                        .expect("Error writing to file");

                let mut temp_functions = functions.clone();

                // sort by the filename then the similarity
                temp_functions.sort_by(|a, b| {
                    let file_cmp = a.file.cmp(&b.file);
                    if file_cmp != std::cmp::Ordering::Equal {
                        return file_cmp;
                    }

                    a.similarity
                        .partial_cmp(&b.similarity)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                for function in &mut temp_functions {
                    // Write function details to file
                    let mut decompiled = true;

                    for pair in &pairs.clone() {
                        if function.file.contains(&pair.path_matcher) {
                            for inc in &pair.include_asm {
                                if function.file == inc.asm_path && inc.line.contains(&function.name) {
                                    decompiled = false;
                                }
                            }
                        }
                    }

                    writeln!(
                        output_file,
                        "| {:<4.2} | {:<8} | {:<35} | {:<2} ",
                        function.similarity, decompiled, function.name, function.file.strip_prefix("../../").unwrap()
                    )
                    .expect("Error writing to file");
                }
            }
        }
    } else {
        for (_, functions) in entries {
            if functions.len() > 1 {
                println!("------------------------");

                for function in functions {
                    println!(
                        "{:.2} {:?} {:?} {:?}",
                        function.similarity, function.decompiled, function.name, function.file
                    );
                }
            }
        }
    }
}

fn do_ordered_compare(dirs: Vec<String>, threshold: f64) {
    let mut files = Vec::new();

    for dir in dirs {
        let mut funcs = Vec::new();
        process_directory(&dir, &mut funcs);

        // sort functions by vram address
        funcs.sort_by_key(|function| {
            function
                .ops
                .first()
                .map_or(u64::MAX, |instr| instr.vram_addr)
        });

        files.push(DupsFile {
            name: dir.to_string(),
            funcs: funcs.clone(),
        });
    }

    for file in &files {
        println!("file {}", file.name);
        for func in &file.funcs {
            println!("\t{} {}", func.name, func.ops.len());
        }
    }

    // 2 way comparison for determining patterns in overlays
    let mut pairs: Vec<Vec<Function>> = Vec::new();

    // print out all found duplicates with their similarity values
    let hyphens = "-".repeat(80);
    println!("{}", hyphens);
    println!("Duplicates and similarity");
    println!("{}", hyphens);

    for func_0 in &files[0].funcs {
        for func_1 in &files[1].funcs {
            let result = levenshtein_similarity(&func_0.key, &func_1.key);

            if result >= threshold {
                println!(
                    "{:<width$} | {:<width$} | {:<width$}",
                    func_0.name,
                    func_1.name,
                    result,
                    width = 40
                );
                let mut temp = Vec::new();
                temp.push(func_0.clone());
                temp.push(func_1.clone());
                pairs.push(temp.clone());
            }
        }
    }

    // print out functions as they are seen in order by the first file. Indicate if it's a
    // duplicate if the second function is non-blank

    println!("{}", hyphens);
    println!("Functions in file order");
    println!("{}", hyphens);
    println!(
        "{:<width$} | {:<width$}",
        files[0].name,
        files[1].name,
        width = 40
    );
    println!("{}", hyphens);

    for func_0 in &files[0].funcs {
        let mut has_dup = false;
        let mut dup_name = "";
        for pair in &pairs {
            if func_0.name == pair[0].name {
                has_dup = true;
                dup_name = &pair[1].name;
            }
        }

        println!("{:<width$} | {:<width$}", func_0.name, dup_name, width = 40);
    }
}

fn main() {
    let args = Args::parse();

    let threshold = args.threshold;
    let dirs = args.dir;
    let output_file = args.output_file;
    let num_dirs = dirs.len();
    let src_base_dir = args.src_base;

    if num_dirs == 2 {
        do_ordered_compare(dirs, threshold);
    } else {
        do_dups_report(output_file, threshold);
    }
}

fn levenshtein_similarity(s1: &[u8], s2: &[u8]) -> f64 {
    let len1 = s1.len();
    let len2 = s2.len();
    let mut dp = vec![vec![0; len2 + 1]; len1 + 1];

    for i in 0..=len1 {
        dp[i][0] = i;
    }

    for j in 0..=len2 {
        dp[0][j] = j;
    }

    for (i, x) in s1.iter().enumerate() {
        for (j, y) in s2.iter().enumerate() {
            dp[i + 1][j + 1] = if x == y {
                dp[i][j]
            } else {
                dp[i][j].min(dp[i][j + 1]).min(dp[i + 1][j]) + 1
            };
        }
    }

    let max_len = len1.max(len2) as f64;
    let result = (max_len - dp[len1][len2] as f64) / max_len;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // two equal strings
    #[test]
    fn test_levenshtein_similarity_1() {
        let s1 = "hello".as_bytes();
        let s2 = "hello".as_bytes();
        let similarity = levenshtein_similarity(s1, s2);
        assert_eq!(similarity, 1.0);
    }

    // almost the same (swap)
    #[test]
    fn test_levenshtein_similarity_09() {
        let s1 = "hello hello hello".as_bytes();
        let s2 = "hello hello hellu".as_bytes();
        let similarity = levenshtein_similarity(s1, s2);
        assert!(similarity >= 0.9);
        assert!(similarity < 1.0);
    }

    // almost the same (insertion)
    #[test]
    fn test_levenshtein_similarity_09_2() {
        let s1 = "hello hello hello".as_bytes();
        let s2 = "hello hell o hello".as_bytes();
        let similarity = levenshtein_similarity(s1, s2);
        assert!(similarity >= 0.9);
        assert!(similarity < 1.0);
    }

    // totally different
    #[test]
    fn test_levenshtein_similarity_0() {
        let s1 = "hello".as_bytes();
        let s2 = "world".as_bytes();
        let similarity = levenshtein_similarity(s1, s2);
        assert_eq!(similarity, 0.2);
    }
}
