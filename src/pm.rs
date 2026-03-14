use crate::instance_config::{Instance, InstancesConfig};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PM_DIR: &str = ".onyx";
const PIDS_DIR: &str = ".onyx/pids";
const LOGS_DIR: &str = ".onyx/logs";

#[derive(Debug)]
pub struct ProcessInfo {
    pub name: String,
    pub pid: Option<u32>,
    pub status: ProcessStatus,
    pub config: String,
    pub port: Option<u16>,
}

#[derive(Debug, PartialEq)]
pub enum ProcessStatus {
    Running,
    Stopped,
    Error,
}

pub fn init_pm_dirs() -> Result<(), String> {
    fs::create_dir_all(PIDS_DIR)
        .map_err(|e| format!("Failed to create PID directory: {}", e))?;
    fs::create_dir_all(LOGS_DIR)
        .map_err(|e| format!("Failed to create logs directory: {}", e))?;
    Ok(())
}

fn pid_file_path(name: &str) -> PathBuf {
    Path::new(PIDS_DIR).join(format!("{}.pid", name))
}

fn log_file_path(name: &str) -> PathBuf {
    Path::new(LOGS_DIR).join(format!("{}.log", name))
}

pub fn read_pid(name: &str) -> Option<u32> {
    let pid_file = pid_file_path(name);
    if !pid_file.exists() {
        return None;
    }

    fs::read_to_string(&pid_file)
        .ok()
        .and_then(|content| content.trim().parse().ok())
}

fn write_pid(name: &str, pid: u32) -> Result<(), String> {
    let pid_file = pid_file_path(name);
    fs::write(&pid_file, pid.to_string())
        .map_err(|e| format!("Failed to write PID file: {}", e))
}

fn remove_pid(name: &str) {
    let pid_file = pid_file_path(name);
    let _ = fs::remove_file(&pid_file);
}

pub fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Send signal 0 to check if process exists
        Command::new("kill")
            .args(&["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        // On Windows, use tasklist to check if process exists
        Command::new("tasklist")
            .args(&["/FI", &format!("PID eq {}", pid), "/NH"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|output| output.contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

pub fn start_instance(instance: &Instance) -> Result<(), String> {
    if !instance.enabled {
        return Err(format!("Instance '{}' is disabled in config", instance.name));
    }

    // Check if already running
    if let Some(pid) = read_pid(&instance.name) {
        if is_process_running(pid) {
            return Err(format!("Instance '{}' is already running (PID: {})", instance.name, pid));
        } else {
            // Stale PID file, clean it up
            remove_pid(&instance.name);
        }
    }

    // Get current executable path
    let exe_path = std::env::current_exe()
        .map_err(|e| format!("Failed to get current executable path: {}", e))?;

    let log_file = log_file_path(&instance.name);
    let log_handle = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .map_err(|e| format!("Failed to open log file: {}", e))?;

    // Build command
    let mut cmd = Command::new(&exe_path);
    cmd.arg("--config")
        .arg(&instance.config)
        .arg("serve");

    if let Some(port) = instance.port {
        cmd.arg("--port").arg(port.to_string());
    }

    // Redirect stdout and stderr to log file
    cmd.stdout(Stdio::from(
        log_handle.try_clone()
            .map_err(|e| format!("Failed to clone log handle: {}", e))?
    ));
    cmd.stderr(Stdio::from(log_handle));

    // Detach from parent process
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Create new process group
                libc::setsid();
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    // Spawn the process
    let child = cmd.spawn()
        .map_err(|e| format!("Failed to spawn process: {}", e))?;

    let pid = child.id();

    // Ensure PID file is written
    let pid_file = pid_file_path(&instance.name);
    println!("DEBUG: Writing PID {} to file: {}", pid, pid_file.display());
    write_pid(&instance.name, pid)?;

    // Verify PID file was created
    if pid_file.exists() {
        println!("DEBUG: PID file created successfully");
    } else {
        println!("DEBUG: WARNING - PID file NOT created!");
    }

    println!("Started instance '{}' (PID: {}, Port: {})",
        instance.name,
        pid,
        instance.port.map(|p| p.to_string()).unwrap_or_else(|| "default".to_string())
    );
    println!("Logs: {}", log_file.display());

    Ok(())
}

pub fn stop_instance(name: &str) -> Result<(), String> {
    let pid = read_pid(name)
        .ok_or_else(|| format!("Instance '{}' is not running", name))?;

    if !is_process_running(pid) {
        remove_pid(name);
        return Err(format!("Instance '{}' is not running (stale PID file)", name));
    }

    // Kill the process
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(&[pid.to_string()])
            .status()
            .map_err(|e| format!("Failed to kill process: {}", e))?;
    }

    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(&["/PID", &pid.to_string(), "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("Failed to kill process: {}", e))?;
    }

    remove_pid(name);
    println!("Stopped instance '{}' (PID: {})", name, pid);

    Ok(())
}

pub fn restart_instance(instance: &Instance) -> Result<(), String> {
    // Try to stop if running (ignore errors if not running)
    let _ = stop_instance(&instance.name);

    // Wait a bit for the process to stop
    std::thread::sleep(std::time::Duration::from_millis(500));

    start_instance(instance)
}

pub fn get_instance_status(instance: &Instance) -> ProcessInfo {
    let pid = read_pid(&instance.name);
    let status = match pid {
        Some(p) if is_process_running(p) => ProcessStatus::Running,
        Some(_) => {
            // Stale PID file
            remove_pid(&instance.name);
            ProcessStatus::Stopped
        }
        None => ProcessStatus::Stopped,
    };

    ProcessInfo {
        name: instance.name.clone(),
        pid,
        status,
        config: instance.config.clone(),
        port: instance.port,
    }
}

pub fn list_instances(instances_config: &InstancesConfig) -> Vec<ProcessInfo> {
    instances_config
        .instances
        .iter()
        .map(get_instance_status)
        .collect()
}

pub fn show_logs(name: &str, lines: usize, follow: bool) -> Result<(), String> {
    let log_file = log_file_path(name);

    if !log_file.exists() {
        return Err(format!("No logs found for instance '{}'", name));
    }

    if follow {
        // Tail -f functionality
        println!("Following logs for instance '{}' (Ctrl+C to stop)...\n", name);

        #[cfg(unix)]
        {
            Command::new("tail")
                .args(&["-f", "-n", &lines.to_string(), log_file.to_str().unwrap()])
                .status()
                .map_err(|e| format!("Failed to tail logs: {}", e))?;
        }

        #[cfg(windows)]
        {
            // Simple follow implementation for Windows
            let file = fs::File::open(&log_file)
                .map_err(|e| format!("Failed to open log file: {}", e))?;
            let reader = BufReader::new(file);
            let mut all_lines: Vec<String> = reader.lines().filter_map(Result::ok).collect();

            // Print last N lines
            let start = if all_lines.len() > lines {
                all_lines.len() - lines
            } else {
                0
            };
            for line in &all_lines[start..] {
                println!("{}", line);
            }

            // Keep reading new lines
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let file = fs::File::open(&log_file)
                    .map_err(|e| format!("Failed to open log file: {}", e))?;
                let reader = BufReader::new(file);
                let new_lines: Vec<String> = reader.lines().filter_map(Result::ok).collect();

                if new_lines.len() > all_lines.len() {
                    for line in &new_lines[all_lines.len()..] {
                        println!("{}", line);
                    }
                    all_lines = new_lines;
                }
            }
        }
    } else {
        // Just show last N lines
        let file = fs::File::open(&log_file)
            .map_err(|e| format!("Failed to open log file: {}", e))?;
        let reader = BufReader::new(file);
        let all_lines: Vec<String> = reader.lines().filter_map(Result::ok).collect();

        let start = if all_lines.len() > lines {
            all_lines.len() - lines
        } else {
            0
        };

        println!("Last {} lines of logs for instance '{}':\n", lines, name);
        for line in &all_lines[start..] {
            println!("{}", line);
        }
    }

    Ok(())
}

pub fn cleanup_stale_pids() {
    if let Ok(entries) = fs::read_dir(PIDS_DIR) {
        for entry in entries.filter_map(Result::ok) {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".pid") {
                    let instance_name = name.trim_end_matches(".pid");
                    if let Some(pid) = read_pid(instance_name) {
                        if !is_process_running(pid) {
                            remove_pid(instance_name);
                        }
                    }
                }
            }
        }
    }
}
