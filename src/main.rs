use regex::Regex;
use serde::Deserialize;
use snafu::prelude::*;
use std::collections::HashMap;
use std::fs::{self, create_dir_all, File};
use std::io::Write;
use std::time::{Duration, SystemTime};
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use clap::Parser;
use rayon::prelude::*;
use indicatif::{ProgressBar, ProgressStyle};


#[derive(Debug,Snafu)]
#[snafu(display("Drawio build error for {output_path:?} : {message}"))]
struct DrawioError {
    message: String,
    input_path: PathBuf,
    output_path: PathBuf,
    stderr: Vec<u8>,
    stdout: Vec<u8>,
    ///If "None" we failed before terminating the program
    exit_code: Option<ExitStatus>,
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
    draft: bool,

    ///Drawio build args. Separate flags and flag value with whitespaces.
    /// /// Don't forget to put the whole thing in quotes
    #[arg(long,default_value="-x -f png -t -s 5")]
    build_args : String,

    ///Path to optional config file
    #[arg(long)]
    config: Option<String>,
}

#[derive(Deserialize,Debug)]
struct DrawioFileConfig {
    ///Name of the file for which this config should be applied
    /// NOT the whole path, just the filename
    name: String,
    ///Specifies the order in which layers should be exported
    ///outer array: export steps, inner array: layers for that step
    /// no append semantics; specify all layers for each step
    order: Vec<Vec<u8>>,
}

/// User specified tweaks for the build process
#[derive(Default,Deserialize,Debug)]
struct DrawioConfig {
    ///Config overrides for individual drawio files
    inidividual_configs : Option<Vec<DrawioFileConfig>>,
}

struct DrawioProcess {
    output_path: PathBuf,
    input_path: PathBuf,
    old_modified_time: Option<SystemTime>,
    handle: Child
}

enum LayerConfig {
    ///Number of layers. Exports [0],[0,1],[0,1,2]...
    Incremental(usize),
    ///For each export step, specify the layers and their oder
    /// Example: [ [0,2],[0,1]] will export layers 0,2 in first step
    /// and layers 0,1 in second step
    Custom(Vec<Vec<u8>>),
}
struct BuildConfig {
    ///general flags that or passed to drawio. DO NOT pass layer configs here
    flags: Vec<String>,
    layer_config: LayerConfig
}


/// Convert LayerConfig to strings that can be passed to the drawio cli
fn assemble_layer_cli_flag(config: &LayerConfig) -> Vec<String> {
    let mut result = Vec::new();
    match config {
        LayerConfig::Incremental(layer_count) => {
            let mut buf: String  = "0".to_string();
            result.push(buf.clone());
            for layer in 1..*layer_count {
                buf += &format!(",{}",layer); 
                result.push(buf.clone());
            }
            result
        },
        LayerConfig::Custom(v) => {
            v.iter().map(|inner| inner.iter().map(|num| format!("{}",num)).collect::<Vec<String>>().join(",")).collect()
        
        },
    }
}

fn run_command(drawio_binary : &str, file: &PathBuf, config: &BuildConfig, out_dir: &str,progress : &ProgressBar) -> Result<(),DrawioError> {
    // Build the command
    let file_name = file.file_stem().unwrap().to_str().unwrap();
    let full_file_path = file.as_path().as_os_str().to_str().unwrap();

    let mut handles = Vec::new();
    let export_steps = assemble_layer_cli_flag(&config.layer_config);
    // Add the file and flags to the command
    for (idx,step) in export_steps.iter().enumerate() {

        let mut command = Command::new(drawio_binary);

        let output_path = Path::new(out_dir).join(format!("{}-{}.png",file_name,idx));

        //skip build if output file is older than input file, i.e. no changes since built
        let mut old_modified_time = None;
        if output_path.exists() {
            let out_modified = output_path.metadata().unwrap().modified().unwrap();
            let in_modified = file.metadata().unwrap().modified().unwrap();
            if out_modified.ge(&in_modified) {
                continue;
            }
            old_modified_time = Some(out_modified);
        }
        
        command.args(&config.flags).arg("-o").arg(&output_path);
        command.arg("--layers");
        command.arg(&step);
        
        command.arg(full_file_path);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.current_dir(env::current_dir().map_err(|e| DrawioError{
            message: format!("failed to spawn drawio process : {:?}",e).to_string(),
            input_path: PathBuf::from(full_file_path),
            output_path: output_path.clone(),
            stderr: Vec::new(),
            stdout: Vec::new(),
            exit_code: None,
        })?);

    
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
                exit_code: None,
            })?,
        });
    }

    for x in handles {
         // Execute the command
         let output = x.handle.wait_with_output().map_err(|e| DrawioError{
            message: format!("process termination error : {:?}",e).to_string(),
            input_path: x.input_path.clone(),
            output_path: x.output_path.clone(),
            stderr: Vec::new(),
            stdout: Vec::new(),
            exit_code: None,
        })?;
        progress.inc(1);
        let mut error_template = DrawioError{
            message: "generic error".to_string(),
            input_path: x.input_path.clone(),
            output_path: x.output_path.clone(),
            stderr: output.stderr,
            stdout: output.stdout,
            exit_code: None,
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


/// Check well known locations and `hint` for drawio binary. Hint is preferred
/// Returns first matching path
fn search_drawio_binary(hint: Option<String>) -> Option<String> {
    let mut candidates = vec![
        //macos
        "/Applications/draw.io.app/Contents/MacOS//draw.io".to_string(),
        //generic 1
        "drawio".to_string(),
        //generic 2
        "draw.io".to_string(),
    ];


    //insert at front is important so that hint is preferred over the other paths
    if let Some(hint) = hint {
        candidates.insert(0, hint);
    }

    for c in &candidates {
        match Command::new(c).arg("--version").output() {
            Ok(_) => return Some(c.to_string()),
            Err(_) => (),
        }
    }

    None
}

fn main() -> Result<(), AppError> {

    let args = Args::parse();

    let  mut drawio_flags : Vec<String> = args.build_args.split(" ").map(|v| v.to_string()).collect();

    //If draft mode, change scale to 1
    if args.draft {
        let mut scale_flag_idx = None;
        for (i,v) in drawio_flags.iter().enumerate() {
            if v == "-s" || v == "--scale" {
                scale_flag_idx = Some(i);
                break;
            }
        }
        if let Some(idx) = scale_flag_idx {
            if drawio_flags.len() < idx + 1 {
                whatever!("Scale flag does not have argument");
            }
            drawio_flags[idx+1] = "1".to_string();
        }
    }

    let config: DrawioConfig = match args.config {
        Some(path) => {
            serde_json::from_reader(File::open(&path).whatever_context::<String,AppError>(format!("Failed to open config file {}",path))?).whatever_context::<&str,AppError>("Failed to parse config file")?

        },
        None => DrawioConfig::default(),
    };


    //Later we need to quickly check if there is a config override for a given file
    let mut file_to_config :HashMap<String, &DrawioFileConfig> = HashMap::new();
    if let Some(overrides) = &config.inidividual_configs {
        for x in overrides {
            file_to_config.insert(x.name.clone(), x);
        }
    }


    let drawio_path = match search_drawio_binary(args.drawio) {
        Some(v) => v,
        None => whatever!("Failed to locate drawio binary. Please specify path with \"--drawio\" cli argument"),
    };


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


        let content = fs::read_to_string(&dir_entry.path()).whatever_context::<std::string::String, AppError>(format!("failed to read file {:?}", &dir_entry.path()))?;

        let layer_count = match layer_re.find_iter(&content).count() {
            0 => 1,
            v => v,
        };

        drawio_files.push((dir_entry.path(),layer_count));

        
    }

    let task_count :usize = drawio_files.iter().map(|(_,steps)| *steps).sum();
    let progress_bar = ProgressBar::new(task_count as u64);
    progress_bar.set_style(ProgressStyle::with_template("[{elapsed}] {wide_bar} {pos:>7}/{len:7} {msg}").expect("progress bar template failed"));
    progress_bar.enable_steady_tick(Duration::from_millis(200));
    progress_bar.inc(0);
    let first_err = drawio_files.par_iter().try_for_each(|(input_path,layer_count)| {
        let file_name = input_path.file_name().expect(&format!("unexpected malformed path {:?}. Should no longer happen at this stage",input_path)).to_str().unwrap().to_string();

        let config = match file_to_config.get(&file_name) {
            Some(custom_config) => {
                BuildConfig{
                    flags: drawio_flags.clone(),
                    layer_config: LayerConfig::Custom(custom_config.order.clone()),
                }
            },
            None => BuildConfig{
                flags: drawio_flags.clone(),
                layer_config: LayerConfig::Incremental(*layer_count),
            },
        };
        run_command(&drawio_path,input_path, &config,&args.output,&progress_bar)
    });
    match first_err {
        Ok(_) => progress_bar.finish_with_message("Built all figures"),
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


#[cfg(test)]
mod test {

    use super::*;

    //fn assemble_layer_flag(config: LayerConfig) -> Vec<String> {

    #[test]
    fn test_assemble_layer_flag_incremental() {
            let want = vec!["0".to_string()];
            let got = assemble_layer_cli_flag(&LayerConfig::Incremental(1));
            assert_eq!(want,got);
            let want = vec!["0".to_string(),"0,1".to_string(),"0,1,2".to_string()];
            let got = assemble_layer_cli_flag(&LayerConfig::Incremental(3));
            assert_eq!(want,got);
    }

    #[test]
    fn test_assemble_layer_flag_custom() {
        let want = vec!["1,0".to_string(),"2,5".to_string()];
        let got = assemble_layer_cli_flag(&LayerConfig::Custom(vec![vec![1,0],vec![2,5]]));
        assert_eq!(want,got);
    }
}