# bpaste: Command-line Uploader for bepasty

`bpaste` is a Rust command-line tool for uploading files or clipboard content to your self-hosted [bepasty](https://bepasty.org/) pastebin server. It is designed for quick sharing of text, files, or clipboard data via a simple terminal command.

## Features
- **Clipboard upload**: By default, uploads the current clipboard contents.
- **File upload**: Specify a file path to upload a file.
- **Stdin upload**: Use `-` as the file argument to upload data from standard input.
- **Configurable**: Supports configuration via command-line options, environment variables, and config files (with XDG discovery).
- **Automatic MIME detection**: Uses libmagic to detect file types.
- **Max file size enforcement**: Prevents accidental uploads of large files.
- **URL to clipboard**: Automatically copies the resulting bepasty URL to your clipboard.

## Installation

### Or install it directly via Cargo:

   ```sh
   cargo install --git https://github.com/u6bkep/bpaste.git
   ```

### Or build from source:

1. **Clone the repository:**
   ```sh
   git clone https://github.com/u6bkep/bpaste.git
   cd copy-upload
   ```
2. **Build with Cargo:**
   ```sh
   cargo build --release
   ```
3. **Run the binary:**
   ```sh
   cargo run --release -- [OPTIONS] [FILE]
   ```
**Or Install via Cargo:**
```sh
   cargo install --path .
```
## Usage

```
bpaste [OPTIONS] [FILE]
```
- If `[FILE]` is omitted, clipboard contents are uploaded.
- If `[FILE]` is `-`, data is read from stdin.
- Otherwise, `[FILE]` is uploaded.

### Options
- `--base-url <URL>`: Override bepasty base URL
- `--api-key <KEY>`: Override bepasty API key
- `--config-path <PATH>`: Explicit path to config file
- `--max-file-size <SIZE>`: Maximum file size (e.g. 10M, 512K)

### Examples
- Upload clipboard:
  ```sh
  bpaste
  ```
- Upload a file:
  ```sh
  bpaste myfile.txt
  ```
- Upload from stdin:
  ```sh
  echo "hello world" | bpaste -
  ```

## Configuration

Configuration is layered (highest priority first):
1. Command-line options
2. Environment variables
3. Config file (if found)
4. Built-in defaults

### Environment Variables
- `BPASTE_API_BASE_URL` – Override base URL
- `BPASTE_API_KEY` – API key (required unless provided elsewhere)
- `BPASTE_MAX_FILE_SIZE` – Maximum file size (e.g. 10M, 512K)
- `BPASTE_CONFIG_PATH` – Explicit path to config file

### Config File Discovery
- `$XDG_CONFIG_HOME/bpaste/bpaste.conf` (fallback: `$HOME/.config/...`)
- Each directory in `$XDG_CONFIG_DIRS` (fallback: `/etc/xdg`) is checked for `bpaste/bpaste.conf`

#### Config File Format
Simple key=value per line, lines starting with `#` are comments.

```
# Bpaste uploader configuration
base_url = https://bepasty.example.org
api_key = abcdef123456
max_file_size = 10M
```

## Requirements
- Rust (stable)
- [bepasty](https://bepasty.org/) server
- API key for your bepasty instance

## License
MIT

## Author
u6bkep

