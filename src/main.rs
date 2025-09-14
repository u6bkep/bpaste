use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use clap::Parser;
use copypasta::{ClipboardContext, ClipboardProvider};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use magic::Cookie;
use human_units::Size;
use std::collections::HashMap;
use std::env;
use std::fmt::Debug;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

const DEFAULT_BASE_URL: &str = "http://localhost:8000";
const DEFAULT_MAX_FILE_SIZE: u64 = 4096;

#[derive(Parser)]
#[command(
    name = "bpaste",
    about = "Upload files or clipboard content to bepasty",
    long_about = r#"Upload files or clipboard content to a bepasty server.

Configuration precedence (highest first):
  1. Command-line options
  2. Environment variables
  3. Config file (if found)
  4. Built-in defaults

Environment variables:
  BPASTE_API_BASE_URL       Override base URL
  BPASTE_API_KEY        API key (required unless provided elsewhere)
  BPASTE_MAX_FILE_SIZE  Maximum file size (e.g. 10M, 512K)
  BPASTE_CONFIG_PATH    Explicit path to config file

Config file discovery (if BPASTE_CONFIG_PATH and --config-path absent):
  $XDG_CONFIG_HOME/bpaste/bpaste.conf (fallback: $HOME/.config/...)
  Each directory in $XDG_CONFIG_DIRS (fallback: /etc/xdg) is checked for bpaste/bpaste.conf

Config file format: simple key=value per line, lines starting with # are comments.

Supported keys:
  base_url      = https://bepasty.example.org
  api_key       = mysecretapikey
  max_file_size = 5M

Example (~/.config/bpaste/bpaste.conf):
  # Bpaste uploader configuration
  base_url = https://bepasty.example.org
  api_key = abcdef123456
  max_file_size = 10M

Units for max_file_size follow human_units crate (K, M, G etc)."#
)]
struct Args {
    /// File to upload, or '-' for stdin
    file: Option<String>,

    /// Override bepasty base URL
    #[arg(long)]
    base_url: Option<String>,

    /// Override bepasty API key
    #[arg(long)]
    api_key: Option<String>,

    /// Path to config file (overrides env / XDG search)
    #[arg(long)]
    config_path: Option<String>,

    #[arg(long, value_parser=clap::value_parser!(Size), help = "Maximum file size in bytes")]
    max_file_size: Option<Size>,
}

struct Config {
    base_url: String,
    api_key: String,
    max_file_size: u64,
}

impl Config {
    fn from_args(args: &Args) -> Result<Self> {
        // Determine config path precedence: CLI > ENV > XDG discovery
        let explicit_config_path = args
            .config_path
            .clone()
            .or_else(|| env::var("BPASTE_CONFIG_PATH").ok());

        let discovered_path = if explicit_config_path.is_none() {
            discover_config_file()
        } else {
            None
        };

        let config_path = explicit_config_path.or(discovered_path);

        let file_cfg = if let Some(p) = config_path {
            match parse_config_file(&p) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("Warning: failed to parse config file {}: {}", p, e);
                    None
                }
            }
        } else {
            None
        };

        // Helper closures to get layered values
        let get_str = |cli: &Option<String>, env_key: &str, file_key: &str, def: &str| -> String {
            cli.clone()
                .or_else(|| env::var(env_key).ok())
                .or_else(|| file_cfg.as_ref().and_then(|c| c.get(file_key).cloned()))
                .unwrap_or_else(|| def.to_string())
        };

        let get_u64 = |cli_opt: &Option<Size>, env_key: &str, file_key: &str, def: u64| -> u64 {
            if let Some(v) = cli_opt {
                (*v).into()
            } else if let Ok(env_v) = env::var(env_key) {
                parse_size_to_u64(&env_v).unwrap_or(def)
            } else if let Some(v) = file_cfg.as_ref().and_then(|c| c.get(file_key)) {
                parse_size_to_u64(v).unwrap_or(def)
            } else {
                def
            }
        };

        let base_url = get_str(&args.base_url, "BPASTE_API_BASE_URL", "base_url", DEFAULT_BASE_URL);
        let api_key  = get_str(&args.api_key,  "BPASTE_API_KEY",  "api_key",  "");

        if api_key.is_empty() {
            return Err(anyhow!("API key not provided (CLI/env/config); cannot proceed"));
        }

        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(anyhow!("Base URL must start with http:// or https://"));
        }

        let max_file_size = get_u64(&args.max_file_size, "BPASTE_MAX_FILE_SIZE", "max_file_size", DEFAULT_MAX_FILE_SIZE);
        if max_file_size == 0 {
            return Err(anyhow!("Maximum file size must be greater than 0"));
        }

        Ok(Config { base_url, api_key, max_file_size })
    }
}

enum InputSource {
    File(String),
    Stdin,
    Clipboard,
}

fn detect_input_source(args: &Args) -> InputSource {
    match &args.file {
        Some(file) if file == "-" => InputSource::Stdin,
        Some(file) => InputSource::File(file.clone()),
        None => InputSource::Clipboard,
    }
}

enum File {
    Bytes(Vec<u8>),
    Path(String),
}

impl File {
    fn len(&self) -> usize {
        match self {
            File::Bytes(bytes) => bytes.len(),
            File::Path(path) => fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0),
        }
    }
    
}

impl Debug for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            File::Bytes(bytes) => write!(f, "\"{}\"", String::from_utf8_lossy(bytes)),
            File::Path(path) => write!(f, "File::Path({}) {{{}}}", path , fs::metadata(path).map(|m| m.len()).unwrap_or(0)),
        }
    }
}

struct FileContent {
    content: File,
    filename: String,
}

fn read_input(source: &InputSource) -> Result<FileContent> {
    match source {
        InputSource::File(path) => {
            let path_object = Path::new(path);
            if !path_object.exists() {
                return Err(anyhow!("File '{}' not found", path));
            }
            if path_object.is_dir() {
                return Err(anyhow!("'{}' is a directory, not a file", path));
            }
            let filename = path_object
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            Ok(FileContent { content: File::Path(path.clone()), filename })
        }
        InputSource::Stdin => {
            let mut content = Vec::new();
            io::stdin().read_to_end(&mut content)?;
            if content.is_empty() {
                return Err(anyhow!("No input provided via stdin"));
            }
            let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
            let filename = format!("stdin-{}", timestamp);
            Ok(FileContent { content: File::Bytes(content), filename })
        }
        InputSource::Clipboard => {
            let mut ctx = ClipboardContext::new()
                .map_err(|_| anyhow!("Failed to access clipboard"))?;
            let content = ctx
                .get_contents()
                .map_err(|_| anyhow!("Failed to read clipboard"))?;
            if content.is_empty() {
                return Err(anyhow!("Clipboard is empty"));
            }
            let content_bytes = content.into_bytes();
            let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
            let filename = format!("clipboard-{}", timestamp);
            Ok(FileContent { content: File::Bytes(content_bytes), filename })
        }
    }
}

struct FileType {
    mime_type: String,
}

fn detect_content_type(file: &FileContent) -> Result<FileType> {
    // Use magic to detect MIME type
    let cookie = Cookie::open(magic::cookie::Flags::ERROR | magic::cookie::Flags::EXTENSION)?;
    let database = &Default::default();
    let cookie = cookie.load(database).map_err(|_| anyhow!("Failed to load magic database"))?;

    

    let mime_type = match &file.content {
        File::Bytes(bytes) => {
            // println!("DEBUG: input buffer: {:X?}", bytes);
            cookie.buffer(bytes).map_err(|_| anyhow!("Failed to detect MIME type from bytes"))?
        },
        File::Path(path) => cookie.file(path).map_err(|_| anyhow!("Failed to detect MIME type from path"))?,
    };

    println!("DEBUG: Detected MIME type: {}", mime_type);

    // println!("DEBUG: cookie database: ");
    // std::io::stdout().flush()?;
    // cookie.list(database)?;
    // std::io::stdout().flush()?;

    return Ok(FileType {
        mime_type: mime_type.to_string(),
    });
    
}

async fn upload_to_bepasty(
    config: &Config,
    file_content: &FileContent,
) -> Result<String> {
    let content_type = detect_content_type(file_content)?;
    let content_size = file_content.content.len();
    if content_size > config.max_file_size as usize {
        return Err(anyhow!(
            "File size exceeds maximum limit of {} bytes",
            config.max_file_size
        ));
    }
    let content_range = format!("bytes 0-{}/{}", content_size - 1, content_size);
    let mut encoded_content: String = String::new();

    // Encode content as base64
    match &file_content.content {
        File::Bytes(bytes) => {
            encoded_content.clone_from(&general_purpose::STANDARD.encode(bytes));

        }
        File::Path(path) => {
            let mut file = fs::File::open(path)
                .map_err(|_| anyhow!("Failed to open file '{}'", path))?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)
                .map_err(|_| anyhow!("Failed to read file '{}'", path))?;
            encoded_content.clone_from(&general_purpose::STANDARD.encode(buffer));
        }
    }
    
    // Prepare headers
    let auth_string = format!("username:{}", config.api_key);
    let auth_encoded = general_purpose::STANDARD.encode(auth_string);
    
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Basic {}", auth_encoded))?,
    );
    headers.insert("Content-Range", HeaderValue::from_str(&content_range)?);
    headers.insert("Content-Filename", HeaderValue::from_str(&file_content.filename)?);
    headers.insert(CONTENT_TYPE, HeaderValue::from_str(&content_type.mime_type)?);

    println!("Uploading {}...", file_content.filename);

    let client = reqwest::Client::new();
    let url = format!("{}/apis/rest/items", config.base_url);
    
    let response = client
        .post(&url)
        .headers(headers)
        .body(encoded_content)
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(anyhow!("Upload failed with status: {}", response.status()));
    }
    
    // Extract Content-Location header
    if let Some(content_location) = response.headers().get("content-location") {
        let location_str = content_location.to_str()?;
        let item_id = location_str
            .split('/')
            .last()
            .ok_or_else(|| anyhow!("Invalid Content-Location header"))?;
        let final_url = format!("{}/{}", config.base_url, item_id);
        Ok(final_url)
    } else {
        Err(anyhow!("No Content-Location header found in response"))
    }
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut ctx = ClipboardContext::new()
        .map_err(|_| anyhow!("Failed to access clipboard"))?;
    ctx.set_contents(text.to_string())
        .map_err(|_| anyhow!("Failed to copy to clipboard"))?;
    Ok(())
}

// Parse key=value config file (simple, supports comments with #)
fn parse_config_file(path: &str) -> Result<HashMap<String, String>> {
    let content = fs::read_to_string(path)
        .map_err(|e| anyhow!("Failed to read config file '{}': {}", path, e))?;
    let mut map = HashMap::new();
    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        } else {
            return Err(anyhow!("Malformed line {} in {}: '{}'", idx + 1, path, line));
        }
    }
    Ok(map)
}

fn parse_size_to_u64(s: &str) -> Option<u64> {
    s.parse::<Size>().ok().map(|sz| sz.into())
}

// Discover config file per XDG (lowest priority). Returns first existing path.
fn discover_config_file() -> Option<String> {
    let home = env::var("HOME").ok();
    let xdg_config_home = env::var("XDG_CONFIG_HOME").ok()
        .or_else(|| home.as_ref().map(|h| format!("{}/.config", h)));
    let mut candidates = Vec::new();

    if let Some(base) = xdg_config_home {
        candidates.push(format!("{}/bpaste/bpaste.conf", base));
    }

    let xdg_config_dirs = env::var("XDG_CONFIG_DIRS").ok().unwrap_or_else(|| "/etc/xdg".to_string());
    for dir in xdg_config_dirs.split(':') {
        if dir.is_empty() { continue; }
        candidates.push(format!("{}/bpaste/bpaste.conf", dir));
    }

    for p in candidates {
        if Path::new(&p).is_file() {
            return Some(p);
        }
    }
    None
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::from_args(&args)?;

    // println!("DEBUG: Config: {:?}", config);
    println!("DEBUG: Max file size: {}", config.max_file_size);
    
    let input_source = detect_input_source(&args);
    let file_content = read_input(&input_source)?;
    println!("DEBUG: Read {} bytes from input", file_content.content.len());
    // println!("DEBUG: read: \n{:?}", file_content.content);
    detect_content_type(&file_content)?;
    
    match upload_to_bepasty(&config, &file_content).await {
        Ok(url) => {
            if let Err(e) = copy_to_clipboard(&url) {
                eprintln!("Warning: Failed to copy to clipboard: {}", e);
                println!("Upload successful! URL: {}", url);
            } else {
                println!("Upload successful! URL copied to clipboard: {}", url);
            }
        }
        Err(e) => {
            eprintln!("Upload failed: {}", e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}
