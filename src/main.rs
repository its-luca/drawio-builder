use regex::Regex;
use snafu::prelude::*;
use std::fs::{self, create_dir_all, File};
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::time::SystemTime;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use clap::Parser;
use rayon::prelude::*;


#[derive(Debug,Snafu)]
#[snafu(display("Drawio build error for {output_path:?} : {message}"))]
struct DrawioError {
    message: String,
    input_path: PathBuf,
    output_path: PathBuf,
    stderr: Vec<u8>,
    stdout: Vec<u8>,
    exit_code: ExitStatus,
}
 
#[derive(Debug, Snafu)]
enum AppError {

    #[snafu(transparent)]
    DrawioError{
        source:DrawioError
    },

    ///catch-all error type
    #[snafu(whatever, display("{message}"))]
    Whatever {
        message: String,
        #[snafu(source(from(Box<dyn std::error::Error>, Some)))]
        source: Option<Box<dyn std::error::Error>>,
    },

}


#[derive(Parser)]
#[command(version,about,long_about=None)]
struct Args {

    ///Path to folder with input files
    #[arg(short,long,default_value="./")]
    input: String,

    ///Path to folder where output gets stored.
    /// Will be created if it does not exist
    #[arg(short,long,default_value="./out")]
    output: String,

    ///Path to drawio binary. Defaults to "drawio"
    #[arg(long)]
    drawio: Option<String>,

    ///If true, use lower resolution for faster latex build times
    #[arg(long,default_value="false")]
    draft: bool
}



struct DrawioProcess {
    output_path: PathBuf,
    input_path: PathBuf,
    old_modified_time: Option<SystemTime>,
    handle: Child
}

fn run_command(drawio_path : &str, file: &PathBuf, flags: &Vec<String>, layer_count: usize, out_dir: &str) -> Result<(),DrawioError> {
    // Build the command
    let file_name = file.file_stem().unwrap().to_str().unwrap();
    let full_file_path = file.as_path().as_os_str().to_str().unwrap();

    let mut handles = Vec::new();
    // Add the file and flags to the command
    let mut layer_flag = Vec::new();
    for layer in 0..layer_count {
        layer_flag.push(format!("{}",layer));        

        let mut command = Command::new(drawio_path);

        let output_path = Path::new(out_dir).join(format!("{}-{}.png",file_name,layer));

        //skip build if output file is older than input file, i.e. no changes since built
        let mut old_modified_time = None;
        if output_path.exists() {
            let out_modified = output_path.metadata().unwrap().modified().unwrap();
            let in_modified = file.metadata().unwrap().modified().unwrap();
            if out_modified.ge(&in_modified) {
                if layer != layer_count-1 {
                    layer_flag.push(",".to_string());
                }
                continue;
            }
            old_modified_time = Some(out_modified);
        }
        
        command.args(flags).arg("-o").arg(&output_path);
        command.arg("--layers");
        command.arg(layer_flag.concat());
        
        command.arg(full_file_path);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.current_dir(env::current_dir().map_err(|e| DrawioError{
            message: format!("failed to spawn drawio process : {:?}",e).to_string(),
            input_path: PathBuf::from(full_file_path),
            output_path: output_path.clone(),
            stderr: Vec::new(),
            stdout: Vec::new(),
            exit_code: ExitStatus::from_raw(-1),
        })?);

        eprintln!("Executing command {:?}",command);
    
        handles.push(DrawioProcess{
            output_path: output_path.clone(),
            input_path: PathBuf::from(full_file_path),
            old_modified_time,
            handle: command.spawn().map_err(|e| DrawioError{
                message: format!("failed to spawn drawio process : {:?}",e).to_string(),
                input_path: PathBuf::from(full_file_path),
                output_path : output_path.clone(),
                stderr: Vec::new(),
                stdout: Vec::new(),
                exit_code: ExitStatus::from_raw(-1),
            })?,
        });
        if layer != layer_count-1 {
            layer_flag.push(",".to_string());
        }
    }

    for x in handles {
         // Execute the command
         let output = x.handle.wait_with_output().map_err(|e| DrawioError{
            message: format!("process termination error : {:?}",e).to_string(),
            input_path: x.input_path.clone(),
            output_path: x.output_path.clone(),
            stderr: Vec::new(),
            stdout: Vec::new(),
            exit_code: ExitStatus::from_raw(-1),
        })?;
        let mut error_template = DrawioError{
            message: "generic error".to_string(),
            input_path: x.input_path.clone(),
            output_path: x.output_path.clone(),
            stderr: output.stderr,
            stdout: output.stdout,
            exit_code: output.status,
        };
         if !output.status.success() {
            error_template.message = "error exit code".to_string();
             return Err(error_template);
         }
       
         //drawio's exit code does not reflect if there has been an error
         //For now, we assume that if the output file got created/updated everything succeeded
         match x.old_modified_time {
            Some(old_modified_time) => {
                let new_modified_time = x.output_path.metadata().unwrap().modified().unwrap();
                if old_modified_time.ge(&new_modified_time) {
                    error_template.message = "output file was not updated".to_string();
                    return Err(error_template)
                }
            },
            //file did not previously exist
            None => {
                if !x.output_path.exists() {
                    error_template.message = "output file was not created".to_string();
                    return Err(error_template);
                }
            },
        };
 
    }
    Ok(())

}

fn main() -> Result<(), AppError> {

    let args = Args::parse();

    let  mut drawio_flags : Vec<String> = Vec::from(["-x".to_string(), "-f".to_string(), "png".to_string(), "-t".to_string()]);
    if !args.draft {
        drawio_flags.push("-s".to_string());
        drawio_flags.push("5".to_string());
    }

    let drawio_path = match args.drawio {
        Some(v) => v,
        None => "drawio".to_string(),
    };

    let _ = Command::new(drawio_path.clone()).arg("--version").output().whatever_context::<&str, AppError>("Failed to locate drawio binary. Please specify path")?;

    create_dir_all(&args.output).whatever_context::<std::string::String, AppError>(format!("Failed to create output dir at {}", &args.output))?;

    let mut drawio_files = Vec::new();
    let layer_re = Regex::new(r#"<mxCell id=".*" value=".*" parent="." />"#).whatever_context::<std::string::String, AppError>("failed to compile layer extraction regexp".to_string())?;
    for dir_entry in fs::read_dir(&args.input).whatever_context::<std::string::String, AppError>(format!("error listing files in folder {}", &args.input))? {
        let dir_entry = dir_entry.whatever_context::<std::string::String, AppError>("".to_string())?;
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


        let content = fs::read_to_string(&dir_entry.path()).whatever_context::<std::string::String, AppError>(format!("failed to read file {:?}", &dir_entry.path()))?;

        let layer_count = match layer_re.find_iter(&content).count() {
            0 => 1,
            v => v,
        };

        drawio_files.push((dir_entry.path(),layer_count));

        
    }

    let first_err = drawio_files.par_iter().try_for_each(|(path,layer_count)| {
        run_command(&drawio_path,path, &drawio_flags, *layer_count,&args.output)
    });
    match first_err {
        Ok(_) => eprintln!("Build all figures"),
        Err(e) => {
            let log_path = PathBuf::from(&args.output).join("drawio-builder-errors.log");
            let mut log_file = File::create(&log_path).whatever_context::<String,AppError>(format!("At least one figure failed to build and we failed to create the error log at {:?}",log_path))?;
            write!(log_file,"Stderr and Stdout when trying to create {:?}\n\n",&e.output_path).whatever_context::<&str,AppError>("Failed to write failed figure's build to log file")?;
            log_file.write_all(&e.stdout).whatever_context::<&str,AppError>("Failed to write stdout of failed figure's build to log file")?;
            log_file.write_all(&e.stderr).whatever_context::<&str,AppError>("Failed to write stderr or failed figure's build to log file")?;
            whatever!("At least one figure failed to build. Error log has been created at {:?}",&log_path);
        },
    }

    Ok(())
}
