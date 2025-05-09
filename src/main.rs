use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use id3::{Tag, TagLike};
use clap::{Parser, Subcommand};
use walkdir::WalkDir;
use anyhow::{Result, Context, bail};
use log::{info, warn, error, debug};
use env_logger::Env;

/// A tool that extracts lyrics from MP3 files and concatenates them into a text file
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Directory containing MP3 files or path to a single MP3 file
    #[arg(short, long)]
    input: String,

    /// Output file path
    #[arg(short, long, default_value = "output.txt")]
    output: String,

    /// Recursively search directories
    #[arg(short, long, default_value_t = false)]
    recursive: bool,

    /// Verbose output
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    /// Include file names in output
    #[arg(short = 'n', long, default_value_t = false)]
    include_names: bool,

    /// Add separator between lyrics
    #[arg(short, long, default_value_t = false)]
    separator: bool,

    /// Separator text (used with --separator)
    #[arg(long, default_value = "---")]
    separator_text: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List all MP3 files found but don't extract lyrics
    List {
        /// Directory containing MP3 files
        #[arg(short, long)]
        input: String,
        
        /// Recursively search directories
        #[arg(short, long, default_value_t = false)]
        recursive: bool,
    },
}

fn main() -> Result<()> {
    // Initialize logger with custom environment
    let env = Env::default().filter_or("RUST_LOG", "info");
    env_logger::init_from_env(env);

    // Parse command line arguments
    let args = Args::parse();
    
    // Set log level
    if args.verbose {
        log::set_max_level(log::LevelFilter::Debug);
    } else {
        log::set_max_level(log::LevelFilter::Info);
    }

    // Process subcommands
    if let Some(cmd) = args.command {
        match cmd {
            Commands::List { input, recursive } => {
                let mp3_files = find_mp3_files(&input, recursive)?;
                for file in mp3_files {
                    println!("{}", file.display());
                }
                return Ok(());
            }
        }
    }

    // Default behavior: extract lyrics and write to output file
    let mp3_files = find_mp3_files(&args.input, args.recursive)?;
    
    if mp3_files.is_empty() {
        bail!("No MP3 files found");
    }
    
    info!("Found {} MP3 file(s)", mp3_files.len());
    
    let lyrics = extract_all_lyrics(&mp3_files, args.include_names, args.separator, &args.separator_text)?;
    write_to_file(&args.output, &lyrics)?;
    
    info!("Lyrics written to {}", args.output);
    Ok(())
}

/// Find MP3 files in the given path
fn find_mp3_files(input_path: &str, recursive: bool) -> Result<Vec<PathBuf>> {
    let path = Path::new(input_path);
    let mut mp3_files = Vec::new();

    if path.is_file() {
        if path.extension().and_then(|ext| ext.to_str()) == Some("mp3") {
            mp3_files.push(path.to_path_buf());
        } else {
            bail!("The specified file is not an MP3 file");
        }
    } else if path.is_dir() {
        let walker = if recursive {
            WalkDir::new(path).into_iter()
        } else {
            WalkDir::new(path).max_depth(1).into_iter()
        };

        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("mp3") {
                mp3_files.push(path.to_path_buf());
                debug!("Found MP3: {}", path.display());
            }
        }
    } else {
        bail!("The specified path does not exist");
    }

    Ok(mp3_files)
}

/// Extract lyrics from all MP3 files
fn extract_all_lyrics(
    mp3_files: &[PathBuf], 
    include_names: bool, 
    add_separator: bool, 
    separator_text: &str
) -> Result<String> {
    let mut all_lyrics = String::new();

    for (index, file_path) in mp3_files.iter().enumerate() {
        if index > 0 && add_separator {
            all_lyrics.push_str(&format!("\n{}\n", separator_text));
        }

        if include_names {
            all_lyrics.push_str(&format!("File: {}\n\n", file_path.display()));
        }

        match extract_lyrics_from_file(file_path) {
            Ok(Some(lyrics)) => {
                all_lyrics.push_str(&lyrics);
                all_lyrics.push('\n');
                info!("Extracted lyrics from {}", file_path.display());
            }
            Ok(None) => {
                warn!("No lyrics found in {}", file_path.display());
                if include_names {
                    all_lyrics.push_str("[No lyrics found]\n");
                }
            }
            Err(e) => {
                error!("Failed to extract lyrics from {}: {}", file_path.display(), e);
                if include_names {
                    all_lyrics.push_str("[Failed to extract lyrics]\n");
                }
            }
        }
    }

    Ok(all_lyrics)
}

/// Extract lyrics from a single MP3 file
fn extract_lyrics_from_file(file_path: &Path) -> Result<Option<String>> {
    let tag = Tag::read_from_path(file_path)
        .with_context(|| format!("Failed to read ID3 tag from {}", file_path.display()))?;
    
    // First check for USLT (Unsynchronized lyrics) frames
    let mut lyrics_iter = tag.lyrics();
    if let Some(lyrics_frame) = lyrics_iter.next() {
        return Ok(Some(lyrics_frame.text.clone()));
    }
    
    // Check for COMM (Comments) frames that might contain lyrics
    if let Some(comment) = tag.comments().find(|c| c.description == "LYRICS") {
        return Ok(Some(comment.text.clone()));
    }
    
    // Check for TXXX (User defined text) frames
    if let Some(text) = tag.extended_texts().find(|t| t.description == "LYRICS") {
        return Ok(Some(text.value.clone()));
    }
    
    // Check common lyric frame IDs
    for frame_id in &["LYRICS", "SYLT", "LYRW", "UNSYNCEDLYRICS"] {
        if let Some(frame) = TagLike::get(&tag, frame_id) {
            if let Some(content) = frame.content().text() {
                return Ok(Some(content.to_string()));
            }
        }
    }
    
    Ok(None)
}

/// Write the extracted lyrics to a file
fn write_to_file(output_path: &str, content: &str) -> Result<()> {
    let mut file = File::create(output_path)
        .with_context(|| format!("Failed to create output file {}", output_path))?;
    
    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write to output file {}", output_path))?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs::{self, File};
    use std::io::Write;

    // Helper function to create a test MP3 file with lyrics
    fn create_test_mp3(dir: &Path, filename: &str, lyrics: Option<&str>) -> PathBuf {
        let file_path = dir.join(filename);
        
        // Create a minimal MP3 file with a valid ID3 tag structure
        let mut file = File::create(&file_path).unwrap();
        
        // ID3v2 header (10 bytes)
        let id3_header = [
            b'I', b'D', b'3',  // ID3 marker
            0x04, 0x00,         // Version 2.4.0
            0x00,               // Flags
            0x00, 0x00, 0x00, 0x0A  // Size (10 bytes following the header)
        ];
        
        // Write headers and some minimal audio-like data
        file.write_all(&id3_header).unwrap();
        // Add 10 bytes of empty padding to match the size in the header
        file.write_all(&[0; 10]).unwrap();
        // Add some MP3-like frame data
        file.write_all(&[0xFF, 0xFB, 0x90, 0x44, 0x00]).unwrap();
        file.flush().unwrap();
        
        // If lyrics are provided, create a tag with lyrics
        if let Some(lyrics_text) = lyrics {
            let mut tag = Tag::new();
            
            // Add an Unsynchronized lyrics frame
            use id3::frame::Lyrics;
            use id3::TagLike;
            
            let lyrics_frame = Lyrics {
                lang: String::from_utf8(vec![b'e', b'n', b'g']).unwrap(),
                description: String::new(),
                text: lyrics_text.to_string(),
            };
            tag.add_frame(lyrics_frame);
            
            tag.write_to_path(&file_path, id3::Version::Id3v24).unwrap();
        }
        
        file_path
    }

    #[test]
    fn test_find_mp3_files_single_file() {
        let temp_dir = tempdir().unwrap();
        let mp3_path = create_test_mp3(temp_dir.path(), "test.mp3", None);
        
        let files = find_mp3_files(mp3_path.to_str().unwrap(), false).unwrap();
        
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], mp3_path);
    }

    #[test]
    fn test_find_mp3_files_directory() {
        let temp_dir = tempdir().unwrap();
        let mp3_path1 = create_test_mp3(temp_dir.path(), "test1.mp3", None);
        let mp3_path2 = create_test_mp3(temp_dir.path(), "test2.mp3", None);
        
        // Create a non-MP3 file
        let txt_path = temp_dir.path().join("test.txt");
        File::create(&txt_path).unwrap();
        
        let files = find_mp3_files(temp_dir.path().to_str().unwrap(), false).unwrap();
        
        assert_eq!(files.len(), 2);
        assert!(files.contains(&mp3_path1));
        assert!(files.contains(&mp3_path2));
    }

    #[test]
    fn test_find_mp3_files_recursive() {
        let temp_dir = tempdir().unwrap();
        let mp3_path1 = create_test_mp3(temp_dir.path(), "test1.mp3", None);
        
        // Create a subdirectory
        let sub_dir = temp_dir.path().join("subdir");
        fs::create_dir(&sub_dir).unwrap();
        let mp3_path2 = create_test_mp3(&sub_dir, "test2.mp3", None);
        
        // Test non-recursive (should find only one file)
        let files_non_recursive = find_mp3_files(temp_dir.path().to_str().unwrap(), false).unwrap();
        assert_eq!(files_non_recursive.len(), 1);
        assert!(files_non_recursive.contains(&mp3_path1));
        
        // Test recursive (should find both files)
        let files_recursive = find_mp3_files(temp_dir.path().to_str().unwrap(), true).unwrap();
        assert_eq!(files_recursive.len(), 2);
        assert!(files_recursive.contains(&mp3_path1));
        assert!(files_recursive.contains(&mp3_path2));
    }

    #[test]
    fn test_extract_lyrics_from_file() {
        let temp_dir = tempdir().unwrap();
        let test_lyrics = "This is a test lyric\nSecond line";
        let mp3_path = create_test_mp3(temp_dir.path(), "test.mp3", Some(test_lyrics));
        
        let lyrics = extract_lyrics_from_file(&mp3_path).unwrap();
        
        assert!(lyrics.is_some());
        assert_eq!(lyrics.unwrap(), test_lyrics);
    }

    #[test]
    fn test_extract_lyrics_no_lyrics() {
        let temp_dir = tempdir().unwrap();
        
        // For this test, we'll create a more valid MP3-like file
        let file_path = temp_dir.path().join("test.mp3");
        let mut file = File::create(&file_path).unwrap();
        
        // ID3v2 header (10 bytes)
        let id3_header = [
            b'I', b'D', b'3',  // ID3 marker
            0x04, 0x00,         // Version 2.4.0
            0x00,               // Flags
            0x00, 0x00, 0x00, 0x0A  // Size (10 bytes following the header)
        ];
        
        // Write headers and some minimal audio-like data
        file.write_all(&id3_header).unwrap();
        // Add 10 bytes of empty padding to match the size in the header
        file.write_all(&[0; 10]).unwrap();
        // Add some MP3-like frame data
        file.write_all(&[0xFF, 0xFB, 0x90, 0x44, 0x00]).unwrap();
        file.flush().unwrap();
        
        // Now try to extract lyrics (should be None because we didn't add any)
        let lyrics = extract_lyrics_from_file(&file_path).unwrap();
        
        assert!(lyrics.is_none());
    }

    #[test]
    fn test_extract_all_lyrics() {
        let temp_dir = tempdir().unwrap();
        let mp3_path1 = create_test_mp3(temp_dir.path(), "test1.mp3", Some("Lyrics for song 1"));
        let mp3_path2 = create_test_mp3(temp_dir.path(), "test2.mp3", Some("Lyrics for song 2"));
        
        let mp3_files = vec![mp3_path1.clone(), mp3_path2.clone()];
        
        // Test without names or separators
        let lyrics1 = extract_all_lyrics(&mp3_files, false, false, "").unwrap();
        assert!(lyrics1.contains("Lyrics for song 1"));
        assert!(lyrics1.contains("Lyrics for song 2"));
        assert!(!lyrics1.contains("File:"));
        
        // Test with names
        let lyrics2 = extract_all_lyrics(&mp3_files, true, false, "").unwrap();
        assert!(lyrics2.contains("File:"));
        assert!(lyrics2.contains(mp3_path1.to_str().unwrap()));
        
        // Test with separator
        let lyrics3 = extract_all_lyrics(&mp3_files, false, true, "---").unwrap();
        assert!(lyrics3.contains("---"));
    }

    #[test]
    fn test_write_to_file() {
        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("output.txt");
        let content = "Test content";
        
        write_to_file(output_path.to_str().unwrap(), content).unwrap();
        
        let read_content = fs::read_to_string(output_path).unwrap();
        assert_eq!(read_content, content);
    }
}