use regex::Regex;
use serde::Deserialize;
use snafu::{prelude::*, ResultExt, Whatever};
use std::fs::{self, File};
use std::os::unix::process::ExitStatusExt;
use std::{env, io};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};
use clap::{Parser,Subcommand};


#[derive(Parser)]
#[command(version,about,long_about=None)]
struct Args {

    ///Path to folder with input files
    #[arg(short,long,default_value="./")]
    input: String,

    ///Path to folder where output gets stored.
    /// Will be created if it does not exist
    #[arg(short,long,default_value="./out")]
    output: String
}

#[derive(Debug, Deserialize)]
struct CommandEntry {
    file: String,
    flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommandList {
    commands: Vec<CommandEntry>,
}

fn run_command(file: PathBuf, flags: &Vec<String>, layer_count: usize, out_dir: &str) -> io::Result<ExitStatus> {
    // Build the command
    let file_name = file.file_stem().unwrap().to_str().unwrap();
    let full_file_path = file.as_path().as_os_str().to_str().unwrap();

    // Add the file and flags to the command
    let mut layer_flag = Vec::new();
    for layer in 0..layer_count {
        layer_flag.push(format!("{}",layer));        

        let mut command = Command::new("/Applications/draw.io.app/Contents/MacOS/draw.io");

        let output_path = Path::new(out_dir).join(format!("{}-{}.png",file_name,layer));

        //skip build if output file is older than input file, i.e. no changes since built
        if output_path.exists() {
            let out_modified = output_path.metadata().unwrap().modified().unwrap();
            let in_modified = file.metadata().unwrap().modified().unwrap();
            if out_modified.ge(&in_modified) {
                if layer != layer_count-1 {
                    layer_flag.push(",".to_string());
                }
                continue;
            }
        }
        
        command.args(flags).arg("-o").arg(output_path);
        command.arg("--layers");
        command.arg(layer_flag.concat());
        
        command.arg(full_file_path);
        command.current_dir(env::current_dir()?);

        eprintln!("Executing command {:?}",command);
    
        // Execute the command
        let output = command.output()?;
        if !output.status.success() {
            return Ok(output.status)
        }
        eprintln!("Output: {:?}",output);

        if layer != layer_count-1 {
            layer_flag.push(",".to_string());
        }
    }

    Ok(ExitStatus::default())

}

fn main() -> Result<(), Whatever> {

    let args = Args::parse();

    let  drawio_flags : Vec<String> = Vec::from(["-x".to_string(), "-f".to_string(), "png".to_string(), "-t".to_string(), "-s".to_string(), "5".to_string()]);


    let layer_re = Regex::new(r#"<mxCell id=".*" value=".*" parent="." />"#).whatever_context("failed to compile layer extraction regexp")?;
    for dir_entry in fs::read_dir(&args.input).whatever_context(format!("error listing files in folder {}", &args.input))? {
        let dir_entry = dir_entry.whatever_context("")?;
        if !dir_entry.path().is_file() {
            continue;
        }
        match dir_entry.path().extension() {
            Some(v) => if v != "drawio" {
                continue;
            },
            None => continue,
        }
        println!("Processing {:?}", &dir_entry);


        let content = fs::read_to_string(&dir_entry.path()).whatever_context(format!("failed to read file {:?}", &dir_entry.path()))?;

        let layer_count = match layer_re.find_iter(&content).count() {
            0 => 1,
            v => v,
        };

       

        let status = run_command(dir_entry.path(), &drawio_flags, layer_count,&args.output).whatever_context(format!("Failed to build figure {:?}",dir_entry.path()))?;
        if !status.success() {
            whatever!("failed to build {:?} : status={}", dir_entry.path(), status)
        }
    }

    Ok(())
}
