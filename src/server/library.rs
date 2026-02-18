use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A loaded interchange libretto file with its parsed metadata.
#[derive(Debug, Clone)]
pub struct LoadedLibretto {
    /// Unique identifier derived from the filename (without extension)
    pub file_id: String,
    /// Path to the source file
    pub path: PathBuf,
    /// Opera title from the interchange metadata
    pub opera_title: String,
    /// Album name (from first track, used for matching)
    pub album: Option<String>,
    /// Artist (from first track)
    pub artist: Option<String>,
    /// Number of tracks
    pub track_count: usize,
    /// The full parsed JSON value
    pub json: Arc<serde_json::Value>,
}

/// Metadata extracted from an interchange file for matching purposes.
/// One entry per unique album name found across all tracks in a file.
#[derive(Debug, Clone)]
struct AlbumEntry {
    /// file_id of the LibrettoLibrary entry
    file_id: String,
    /// Normalized album name (lowercased, trimmed)
    normalized_album: String,
}

/// Result of a track match against the library.
#[derive(Debug, Clone, Serialize)]
pub struct TrackMatch {
    /// file_id of the matched libretto
    pub file_id: String,
    /// Index of the matched track within the interchange file
    pub track_index: usize,
    /// How the match was made
    pub match_method: MatchMethod,
}

#[derive(Debug, Clone, Serialize)]
pub enum MatchMethod {
    /// Matched by album name
    Album,
    /// Matched by album + track title
    AlbumAndTitle,
    /// Matched by album + disc/track number
    AlbumAndNumber,
}

/// Extracted libretto content for a matched track, sent to the SPA.
/// The SPA is a pure renderer — all matching and extraction happens server-side.
#[derive(Debug, Clone, Serialize)]
pub struct LibrettoContent {
    /// file_id of the matched libretto (for debugging/reference)
    pub file_id: String,
    /// Index of the matched track (for debugging/reference)
    pub track_index: usize,
    /// How the match was made
    pub match_method: MatchMethod,
    /// Track title from the interchange file
    pub track_title: String,
    /// Track ID from the interchange file
    pub track_id: Option<String>,
    /// Act label (e.g., "1", "2")
    pub act: Option<String>,
    /// Album name from the interchange file
    pub album: Option<String>,
    /// Artist from the interchange file
    pub artist: Option<String>,
    /// Opera title from the interchange metadata
    pub opera_title: Option<String>,
    /// The track's timed segments — passed through as raw JSON from the interchange
    pub segments: Vec<serde_json::Value>,
}

/// The libretto library: holds all loaded interchange files and an index for matching.
#[derive(Debug)]
pub struct LibrettoLibrary {
    /// All loaded librettos, keyed by file_id
    pub librettos: HashMap<String, LoadedLibretto>,
    /// Index: normalized album name → list of file_ids that contain tracks for that album
    album_index: HashMap<String, Vec<AlbumEntry>>,
}

impl LibrettoLibrary {
    /// Create an empty library.
    pub fn new() -> Self {
        Self {
            librettos: HashMap::new(),
            album_index: HashMap::new(),
        }
    }

    /// Scan a directory recursively for *.interchange.json files and load them.
    pub fn from_directory(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut library = Self::new();

        if !dir.exists() {
            return Err(format!("Libretto directory does not exist: {}", dir.display()).into());
        }
        if !dir.is_dir() {
            return Err(format!("Libretto path is not a directory: {}", dir.display()).into());
        }

        let files = find_interchange_files(dir)?;
        log::info!("Found {} interchange files in {}", files.len(), dir.display());

        for path in files {
            match library.load_file(&path) {
                Ok(file_id) => {
                    let entry = &library.librettos[&file_id];
                    log::info!("  Loaded {}: \"{}\" ({} tracks, album: {:?})",
                        file_id, entry.opera_title, entry.track_count,
                        entry.album.as_deref().unwrap_or("unknown"));
                }
                Err(e) => {
                    log::warn!("  Failed to load {}: {}", path.display(), e);
                }
            }
        }

        log::info!("Library: {} files loaded, {} album index entries",
            library.librettos.len(), library.album_index.len());

        Ok(library)
    }

    /// Load a single interchange JSON file into the library.
    pub fn load_file(&mut self, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&contents)?;

        // Extract file_id from filename
        let file_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                // Strip .interchange suffix if the file is name.interchange.json
                s.strip_suffix(".interchange").unwrap_or(s).to_string()
            })
            .ok_or_else(|| format!("Cannot determine file_id from path: {}", path.display()))?;

        // Extract metadata
        let opera_title = value
            .get("opera")
            .and_then(|o| o.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown Opera")
            .to_string();

        let tracks = value
            .get("tracks")
            .and_then(|t| t.as_array());

        let track_count = tracks.map(|a| a.len()).unwrap_or(0);

        // Extract album/artist from recording block (preferred) or first track (fallback)
        let recording = value.get("recording");
        let album = recording
            .and_then(|r| r.get("album_title"))
            .and_then(|a| a.as_str())
            .map(String::from)
            .or_else(|| {
                // Fallback: scan tracks for album field (old interchange format)
                tracks.and_then(|tracks| {
                    tracks.iter().find_map(|t| t.get("album").and_then(|a| a.as_str()).map(String::from))
                })
            });
        let artist = recording
            .and_then(|r| r.get("artist"))
            .and_then(|a| a.as_str())
            .map(String::from)
            .or_else(|| {
                tracks.and_then(|tracks| {
                    tracks.iter().find_map(|t| t.get("artist").and_then(|a| a.as_str()).map(String::from))
                })
            });

        // Build album index from recording.album_title + recording.album_title_aliases
        // Falls back to scanning per-track album fields for old-format files
        let mut seen_albums = std::collections::HashSet::new();
        let mut index_album = |name: &str| {
            let normalized = normalize_album(name);
            if seen_albums.insert(normalized.clone()) {
                self.album_index
                    .entry(normalized.clone())
                    .or_insert_with(Vec::new)
                    .push(AlbumEntry {
                        file_id: file_id.clone(),
                        normalized_album: normalized,
                    });
            }
        };

        if let Some(rec) = recording {
            if let Some(title) = rec.get("album_title").and_then(|a| a.as_str()) {
                index_album(title);
            }
            if let Some(aliases) = rec.get("album_title_aliases").and_then(|a| a.as_array()) {
                for alias in aliases {
                    if let Some(s) = alias.as_str() {
                        index_album(s);
                    }
                }
            }
        } else if let Some(tracks) = tracks {
            // Fallback for old interchange format without recording block
            for track in tracks {
                if let Some(album_name) = track.get("album").and_then(|a| a.as_str()) {
                    index_album(album_name);
                }
            }
        }

        let loaded = LoadedLibretto {
            file_id: file_id.clone(),
            path: path.to_path_buf(),
            opera_title,
            album,
            artist,
            track_count,
            json: Arc::new(value),
        };

        self.librettos.insert(file_id.clone(), loaded);
        Ok(file_id)
    }

    /// Match a currently-playing track to a libretto in the library.
    ///
    /// Strategy:
    /// 1. Try album name match to narrow to a specific file
    /// 2. Within that file, match by disc_number + track_number, or by title
    pub fn match_track(
        &self,
        album: Option<&str>,
        track_title: Option<&str>,
        disc_number: Option<u32>,
        track_number: Option<u32>,
    ) -> Option<TrackMatch> {
        log::debug!("match_track: album={:?}, title={:?}, disc={:?}, track_num={:?}",
            album, track_title, disc_number, track_number);

        // Step 1: Find candidate files by album name
        let normalized_album = album.map(normalize_album);
        let mut candidate_file_ids: Vec<&str> = if let Some(ref norm) = normalized_album {
            // Try exact match first
            let exact = self.album_index
                .get(norm)
                .map(|entries| entries.iter().map(|e| e.file_id.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            if !exact.is_empty() {
                log::debug!("  album exact match: {:?} -> {:?}", norm, exact);
                exact
            } else {
                // Fuzzy: check if the Roon album contains or is contained by an indexed album
                let fuzzy: Vec<&str> = self.album_index.iter()
                    .filter(|(indexed, _)| norm.contains(indexed.as_str()) || indexed.contains(norm.as_str()))
                    .flat_map(|(_, entries)| entries.iter().map(|e| e.file_id.as_str()))
                    .collect();
                if !fuzzy.is_empty() {
                    log::debug!("  album fuzzy match: {:?} -> {:?}", norm, fuzzy);
                }
                fuzzy
            }
        } else {
            // No album info — try all files
            self.librettos.keys().map(|s| s.as_str()).collect()
        };
        candidate_file_ids.dedup();

        let album_matched = !candidate_file_ids.is_empty();
        if !album_matched {
            // Album didn't match (common with box sets like "Solti - Mozart - The Operas").
            // Fall back to trying all files — title matching can still succeed
            // because Roon titles include the opera name.
            log::debug!("  no album candidates; falling back to all {} file(s). index keys: {:?}",
                self.librettos.len(), self.album_index.keys().collect::<Vec<_>>());
            candidate_file_ids = self.librettos.keys().map(|s| s.as_str()).collect();
        }

        // Step 2: Within candidate files, find the best track match
        for file_id in &candidate_file_ids {
            if let Some(libretto) = self.librettos.get(*file_id) {
                if let Some(tracks) = libretto.json.get("tracks").and_then(|t| t.as_array()) {
                    // Try disc_number + track_number match first (most reliable)
                    // but ONLY if album was matched — disc/track numbers are ambiguous across operas
                    if album_matched {
                        if let (Some(disc), Some(track_num)) = (disc_number, track_number) {
                            for (i, track) in tracks.iter().enumerate() {
                                let t_disc = track.get("disc_number").and_then(|d| d.as_u64()).map(|d| d as u32);
                                let t_num = track.get("track_number").and_then(|n| n.as_u64()).map(|n| n as u32);
                                if t_disc == Some(disc) && t_num == Some(track_num) {
                                    return Some(TrackMatch {
                                        file_id: file_id.to_string(),
                                        track_index: i,
                                        match_method: MatchMethod::AlbumAndNumber,
                                    });
                                }
                            }
                        }
                    }

                    // Title matching (always tried — works across album mismatches)
                    if let Some(title) = track_title {
                        let norm_title = normalize_title(title);
                        for (i, track) in tracks.iter().enumerate() {
                            if let Some(t_title) = track.get("title").and_then(|t| t.as_str()) {
                                if titles_match(&norm_title, &normalize_title(t_title)) {
                                    return Some(TrackMatch {
                                        file_id: file_id.to_string(),
                                        track_index: i,
                                        match_method: MatchMethod::AlbumAndTitle,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        log::debug!("  no track match found in {} candidate file(s)", candidate_file_ids.len());
        None
    }

    /// Extract the full libretto content for a matched track.
    /// Returns the segments and metadata needed by the SPA renderer.
    pub fn extract_track_content(&self, track_match: &TrackMatch) -> Option<LibrettoContent> {
        let libretto = self.librettos.get(&track_match.file_id)?;
        let tracks = libretto.json.get("tracks")?.as_array()?;
        let track = tracks.get(track_match.track_index)?;

        let segments = track.get("segments")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();

        Some(LibrettoContent {
            file_id: track_match.file_id.clone(),
            track_index: track_match.track_index,
            match_method: track_match.match_method.clone(),
            track_title: track.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            track_id: track.get("track_id").and_then(|t| t.as_str()).map(|s| s.to_string()),
            act: track.get("act").and_then(|a| a.as_str()).map(|s| s.to_string()),
            album: libretto.json.get("recording")
                .and_then(|r| r.get("album_title"))
                .and_then(|a| a.as_str())
                .map(|s| s.to_string())
                .or_else(|| track.get("album").and_then(|a| a.as_str()).map(|s| s.to_string())),
            artist: libretto.json.get("recording")
                .and_then(|r| r.get("artist"))
                .and_then(|a| a.as_str())
                .map(|s| s.to_string())
                .or_else(|| track.get("artist").and_then(|a| a.as_str()).map(|s| s.to_string())),
            opera_title: libretto.json.get("opera")
                .and_then(|o| o.get("title"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(libretto.opera_title.clone())),
            segments,
        })
    }

    /// Match a track and extract its content in one step.
    /// Convenience method combining match_track + extract_track_content.
    pub fn match_and_extract(
        &self,
        album: Option<&str>,
        track_title: Option<&str>,
        disc_number: Option<u32>,
        track_number: Option<u32>,
    ) -> Option<LibrettoContent> {
        let track_match = self.match_track(album, track_title, disc_number, track_number)?;
        self.extract_track_content(&track_match)
    }

    /// Get all loaded file summaries (for /librettos endpoint).
    pub fn list_files(&self) -> Vec<LibrettoSummary> {
        let mut summaries: Vec<_> = self.librettos.values().map(|l| {
            LibrettoSummary {
                file_id: l.file_id.clone(),
                opera_title: l.opera_title.clone(),
                album: l.album.clone(),
                artist: l.artist.clone(),
                track_count: l.track_count,
            }
        }).collect();
        summaries.sort_by(|a, b| a.file_id.cmp(&b.file_id));
        summaries
    }
}

/// Summary info about a loaded libretto file.
#[derive(Debug, Clone, Serialize)]
pub struct LibrettoSummary {
    pub file_id: String,
    pub opera_title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub track_count: usize,
}

/// Recursively find all *.interchange.json files in a directory.
fn find_interchange_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    find_interchange_files_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn find_interchange_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            find_interchange_files_recursive(&path, files)?;
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".interchange.json") {
                files.push(path);
            }
        }
    }
    Ok(())
}

/// Normalize an album name for matching: lowercase, collapse whitespace, strip punctuation.
fn normalize_album(album: &str) -> String {
    album
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize a track title for matching.
fn normalize_title(title: &str) -> String {
    // Strip common opera prefix patterns like "Le nozze di Figaro, K. 492, Act I: "
    let stripped = strip_opera_prefix(title);
    stripped
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '"' || *c == '\'' || *c == '.')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip the "Opera Name, K.nnn, Act N:" prefix that Roon prepends to track titles.
fn strip_opera_prefix(title: &str) -> &str {
    // Look for patterns like "..., Act I:" or "..., Act II:" etc.
    // and take everything after the colon
    if let Some(colon_pos) = title.find(": ") {
        // Verify this looks like an opera prefix (contains "Act" before the colon)
        let before_colon = &title[..colon_pos];
        if before_colon.contains("Act ") || before_colon.contains("Akt ") || before_colon.contains("Atto ") {
            return &title[colon_pos + 2..];
        }
    }
    title
}

/// Check if two normalized titles match.
/// Uses progressive matching: exact → prefix → number+keywords → quoted text.
fn titles_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Check if one is a prefix of the other (Roon may truncate)
    if a.starts_with(b) || b.starts_with(a) {
        return true;
    }
    // Check if both have the same "No. N" prefix — strong signal within a single file
    if let (Some(a_num), Some(b_num)) = (extract_number_prefix(a), extract_number_prefix(b)) {
        if a_num == b_num {
            // Same number — check for at least one shared keyword to confirm
            // Strip quotes from words since normalize_title preserves them for extract_quoted
            let strip_quotes = |w: &str| w.replace('"', "").replace('\'', "");
            let a_words: std::collections::HashSet<String> = a.split_whitespace()
                .map(|w| strip_quotes(w))
                .filter(|w| w.len() >= 4)
                .collect();
            let b_words: std::collections::HashSet<String> = b.split_whitespace()
                .map(|w| strip_quotes(w))
                .filter(|w| w.len() >= 4)
                .collect();
            let shared = a_words.intersection(&b_words).count();
            if shared >= 1 {
                return true;
            }
        }
    }
    // Check for special first track: "sinfonia" / "overture" / "ouverture"
    let overture_words = ["sinfonia", "overture", "ouverture", "ouvertura", "preludio"];
    let a_is_overture = overture_words.iter().any(|w| a.contains(w));
    let b_is_overture = overture_words.iter().any(|w| b.contains(w));
    if a_is_overture && b_is_overture {
        return true;
    }
    // Check for quoted text match (e.g., both contain "cinque...dieci...venti")
    let a_quoted = extract_quoted(a);
    let b_quoted = extract_quoted(b);
    if let (Some(aq), Some(bq)) = (a_quoted, b_quoted) {
        if aq == bq && aq.len() >= 5 {
            return true;
        }
    }
    // Substring fallback: strip number prefixes and quotes, then check if one title's
    // core text is contained in the other. This handles Roon disc track numbers differing
    // from opera numbering (e.g., Roon "No. 10" vs interchange "No. 9" for "Non più andrai").
    let strip_num = |s: &str| -> String {
        let s = s.replace('"', "").replace('\'', "");
        // Strip leading "no. N" or similar
        if let Some(prefix) = extract_number_prefix(&s) {
            s[prefix.len()..].trim().to_string()
        } else {
            s.trim().to_string()
        }
    };
    let a_core = strip_num(a);
    let b_core = strip_num(b);
    // The shorter one must be at least 8 chars to avoid trivially short matches
    let (shorter, longer) = if a_core.len() <= b_core.len() { (&a_core, &b_core) } else { (&b_core, &a_core) };
    if shorter.len() >= 8 && longer.contains(shorter.as_str()) {
        return true;
    }
    false
}

/// Extract the "No. N" number prefix from a normalized title.
/// Handles patterns like "no. 1", "no. 12", "nr. 3", "n. 5".
fn extract_number_prefix(title: &str) -> Option<String> {
    // Look for "no. N" or "nr. N" or "n. N" at the start
    let patterns = ["no. ", "nr. ", "n. "];
    for pat in &patterns {
        if title.starts_with(pat) {
            let rest = &title[pat.len()..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !num.is_empty() {
                return Some(format!("{}{}", pat, num));
            }
        }
    }
    None
}

/// Extract text between first pair of quotes in a string.
fn extract_quoted(s: &str) -> Option<&str> {
    let start = s.find('"').or_else(|| s.find('\''))?;
    let quote_char = s.as_bytes()[start] as char;
    let rest = &s[start + 1..];
    let end = rest.find(quote_char)?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_album() {
        assert_eq!(normalize_album("Mozart: Le Nozze di Figaro"), "mozart le nozze di figaro");
        assert_eq!(normalize_album("  Verdi:  La Traviata  "), "verdi la traviata");
    }

    #[test]
    fn test_strip_opera_prefix() {
        assert_eq!(
            strip_opera_prefix("Le nozze di Figaro, K. 492, Act I: No. 1, Duet. Cinque, dieci"),
            "No. 1, Duet. Cinque, dieci"
        );
        assert_eq!(
            strip_opera_prefix("No. 1, Duet. Cinque, dieci"),
            "No. 1, Duet. Cinque, dieci"
        );
        assert_eq!(
            strip_opera_prefix("Die Zauberflöte, K. 620, Akt II: No. 10, Arie"),
            "No. 10, Arie"
        );
    }

    #[test]
    fn test_normalize_title() {
        let roon = "Le nozze di Figaro, K. 492, Act I: No. 1, Duet. Cinque, dieci";
        let interchange = "No. 1 Duetto \"Cinque...dieci...venti\"";
        let rn = normalize_title(roon);
        let it = normalize_title(interchange);
        // Both should start with "no. 1"
        assert!(rn.starts_with("no. 1"));
        assert!(it.starts_with("no. 1"));
    }

    #[test]
    fn test_extract_quoted() {
        assert_eq!(extract_quoted(r#"No. 1 Duetto "Cinque...dieci""#), Some("Cinque...dieci"));
        assert_eq!(extract_quoted("No quotes here"), None);
    }

    #[test]
    fn test_titles_match_prefix() {
        assert!(titles_match("no. 1 duet. cinque dieci", "no. 1 duet. cinque dieci venti"));
        assert!(titles_match("no. 1 duet. cinque dieci venti", "no. 1 duet. cinque dieci"));
    }

    #[test]
    fn test_file_id_from_filename() {
        let path = PathBuf::from("/some/dir/georg-solti-1981.interchange.json");
        let file_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.strip_suffix(".interchange").unwrap_or(s).to_string())
            .unwrap();
        assert_eq!(file_id, "georg-solti-1981");
    }

    #[test]
    fn test_empty_library() {
        let lib = LibrettoLibrary::new();
        assert!(lib.librettos.is_empty());
        assert!(lib.match_track(Some("anything"), None, None, None).is_none());
    }

    #[test]
    fn test_load_and_match() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("roon-rd-test-library");
        std::fs::create_dir_all(&dir).unwrap();

        // Create a minimal interchange file
        let file_path = dir.join("test-recording.interchange.json");
        let mut f = std::fs::File::create(&file_path).unwrap();
        write!(f, r#"{{
            "version": "1.0",
            "opera": {{ "title": "Test Opera", "composer": "Test", "language": "it" }},
            "tracks": [
                {{
                    "track_id": "d1t1",
                    "title": "Overture",
                    "album": "Test Opera Recording",
                    "artist": "Conductor / Orchestra",
                    "disc_number": 1,
                    "track_number": 1,
                    "duration_seconds": 300.0,
                    "segments": []
                }},
                {{
                    "track_id": "d1t2",
                    "title": "No. 1 Aria \"Bella voce\"",
                    "album": "Test Opera Recording",
                    "disc_number": 1,
                    "track_number": 2,
                    "duration_seconds": 240.0,
                    "segments": [{{ "start": 0.0, "text": "Hello" }}]
                }}
            ]
        }}"#).unwrap();
        drop(f);

        let lib = LibrettoLibrary::from_directory(&dir).unwrap();
        assert_eq!(lib.librettos.len(), 1);

        // Match by album + disc/track number
        let m = lib.match_track(
            Some("Test Opera Recording"),
            Some("Overture"),
            Some(1), Some(1),
        );
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.file_id, "test-recording");
        assert_eq!(m.track_index, 0);

        // Match by album + track number for second track
        let m = lib.match_track(
            Some("Test Opera Recording"),
            None,
            Some(1), Some(2),
        );
        assert!(m.is_some());
        assert_eq!(m.unwrap().track_index, 1);

        // No match for wrong album
        let m = lib.match_track(
            Some("Completely Different Album"),
            None,
            Some(1), Some(1),
        );
        assert!(m.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
