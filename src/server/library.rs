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

                    // Title matching using parsed title structure (scored, picks best)
                    if let Some(title) = track_title {
                        let roon_parsed = parse_roon_title(title);
                        let mut best_score = 0u32;
                        let mut best_index = None;
                        for (i, track) in tracks.iter().enumerate() {
                            if let Some(t_title) = track.get("title").and_then(|t| t.as_str()) {
                                let ic_parsed = parse_ic_title(t_title);
                                if let Some(score) = parsed_titles_match(&roon_parsed, &ic_parsed) {
                                    if score > best_score {
                                        best_score = score;
                                        best_index = Some(i);
                                    }
                                }
                            }
                        }
                        if let Some(idx) = best_index {
                            log::debug!("  title match: file={} track={} score={}", file_id, idx, best_score);
                            return Some(TrackMatch {
                                file_id: file_id.to_string(),
                                track_index: idx,
                                match_method: MatchMethod::AlbumAndTitle,
                            });
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

/// Parsed representation of a track title, extracted from either Roon or interchange format.
/// Contains structured fields that can be compared independently for scoring.
#[derive(Debug, Clone)]
struct ParsedTitle {
    /// Act number: "1", "2", "3", "4"
    act: Option<String>,
    /// Opera number (e.g., 1 from "No. 1")
    number: Option<u32>,
    /// Type label normalized: "duet", "aria", "trio", "finale", "recitativo", etc.
    type_label: Option<String>,
    /// Normalized text fragments — the actual sung/spoken text
    text_fragments: Vec<String>,
}

/// Normalize a text fragment for comparison: lowercase, keep only alphanumeric + spaces.
fn normalize_text(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a Roon track title into structured components.
///
/// Roon opera titles typically look like:
///   "Le nozze di Figaro, K. 492, Act I: No. 1, Duet. Cinque, dieci, venti \u{2013} Cosa stai misurando"
///   "Le nozze di Figaro, K. 492, Act II: Recit. Che novità!"
///   "Le nozze di Figaro, K. 492: Overture"
fn parse_roon_title(raw: &str) -> ParsedTitle {
    let mut act = None;
    let mut number = None;
    let mut type_label = None;

    // Extract act from the opera prefix before stripping
    for (prefix, act_val) in &[
        ("Act IV", "4"), ("Act III", "3"), ("Act II", "2"), ("Act I", "1"),
        ("Akt IV", "4"), ("Akt III", "3"), ("Akt II", "2"), ("Akt I", "1"),
        ("Atto IV", "4"), ("Atto III", "3"), ("Atto II", "2"), ("Atto I", "1"),
    ] {
        if raw.contains(prefix) {
            act = Some(act_val.to_string());
            break;
        }
    }

    // Strip opera prefix (e.g., "Le nozze di Figaro, K. 492, Act I: ")
    let stripped = strip_opera_prefix(raw);

    // Extract first "No. N" found anywhere
    let mut s = stripped.to_string();
    if let Some(pos) = s.find("No. ") {
        let after = &s[pos + 4..];
        let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<u32>() {
            number = Some(n);
        }
    }

    // Remove all "No. N," or "No. N " occurrences to get to the text
    while let Some(pos) = s.find("No. ") {
        let rest = &s[pos + 4..];
        let digits_end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let mut remove_end = pos + 4 + digits_end;
        // Also consume trailing comma and/or space
        let remaining = &s[remove_end..];
        if remaining.starts_with(", ") {
            remove_end += 2;
        } else if remaining.starts_with(',') {
            remove_end += 1;
        } else if remaining.starts_with(' ') {
            remove_end += 1;
        }
        s = format!("{}{}", &s[..pos], &s[remove_end..]);
    }

    // Extract and remove type labels (longest first to avoid partial matches)
    let labels = [
        ("Recitativo ed Aria:", "recitativo ed aria"),
        ("Recit.", "recitativo"),
        ("Duet.", "duet"),
        ("Trio.", "trio"),
        ("Sextet.", "sextet"),
        ("Chorus.", "chorus"),
        ("Cavatina:", "cavatina"),
        ("Aria.", "aria"),
        ("Finale:", "finale"),
    ];
    for (label, norm) in &labels {
        if s.contains(label) {
            if type_label.is_none() {
                type_label = Some(norm.to_string());
            }
            s = s.replace(label, "");
        }
    }

    // Remove finale part labels like "a-c." "d-g." "e-g."
    let mut cleaned = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len()
            && chars[i].is_ascii_lowercase()
            && chars[i + 1] == '-'
            && chars[i + 2].is_ascii_lowercase()
        {
            i += 3; // skip "a-c"
            if i < chars.len() && chars[i] == '.' {
                i += 1; // skip "."
            }
            while i < chars.len() && chars[i] == ' ' {
                i += 1; // skip trailing spaces
            }
        } else {
            cleaned.push(chars[i]);
            i += 1;
        }
    }
    s = cleaned;

    // Split on em-dash ' \u{2013} ' or regular dash ' - ' to get text fragments
    let fragments: Vec<String> = s
        .split(" \u{2013} ")
        .flat_map(|part| part.split(" - "))
        .map(|f| normalize_text(f.trim()))
        .filter(|f| f.len() >= 3)
        .collect();

    ParsedTitle { act, number, type_label, text_fragments: fragments }
}

/// Parse an interchange file track title into structured components.
///
/// IC titles look like:
///   'No. 1 Duetto "Cinque...dieci...venti" (Figaro, Susanna); recitativo "Cosa stai misurando"'
///   'Recitativo "Che novità!" (Conte, Contessa)'
///   'No. 15 Finale, Part 1: "Esci, ormai, garzon malnato" (Conte, Contessa, Susanna)'
///   'Sinfonia'
fn parse_ic_title(raw: &str) -> ParsedTitle {
    let mut number = None;
    let mut type_label = None;

    // Extract "No. N"
    if let Some(pos) = raw.find("No. ") {
        let after = &raw[pos + 4..];
        let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<u32>() {
            number = Some(n);
        }
    }

    // Extract type label from IC format
    let lc = raw.to_lowercase();
    let type_patterns = [
        ("finale", "finale"),
        ("duettino", "duet"), ("duetto", "duet"),
        ("terzetto", "trio"),
        ("sestetto", "sextet"),
        ("cavatina", "cavatina"),
        ("canzona", "aria"),
        ("aria", "aria"),
        ("coro", "chorus"),
        ("recitativo", "recitativo"),
    ];
    for (pattern, norm) in &type_patterns {
        if lc.contains(pattern) {
            type_label = Some(norm.to_string());
            break;
        }
    }

    // Extract all quoted text fragments
    let mut fragments = Vec::new();
    let mut search = raw;
    while let Some(start) = search.find('"') {
        let rest = &search[start + 1..];
        if let Some(end) = rest.find('"') {
            let quoted = &rest[..end];
            let normalized = normalize_text(quoted);
            if normalized.len() >= 3 {
                fragments.push(normalized);
            }
            search = &rest[end + 1..];
        } else {
            break;
        }
    }

    // If no quoted text found, normalize the whole title as a single fragment
    // (handles "Sinfonia" and similar simple titles)
    if fragments.is_empty() {
        let normalized = normalize_text(raw);
        if normalized.len() >= 3 {
            fragments.push(normalized);
        }
    }

    // IC titles don't usually embed act info (it's in a separate JSON field)
    ParsedTitle { act: None, number, type_label, text_fragments: fragments }
}

/// Score how well a roon text fragment matches against a set of IC text fragments.
/// Returns the best score found (0 if no match).
fn best_fragment_score(roon_frag: &str, ic_fragments: &[String]) -> u32 {
    let rw: Vec<&str> = roon_frag.split_whitespace().collect();
    let mut best = 0u32;

    for ic_frag in ic_fragments {
        let iw: Vec<&str> = ic_frag.split_whitespace().collect();

        // 3-word prefix of roon fragment found in IC fragment
        if rw.len() >= 3 {
            let probe = format!("{} {} {}", rw[0], rw[1], rw[2]);
            if probe.len() >= 6 && ic_frag.contains(&probe) {
                best = best.max(3);
                continue;
            }
        }
        // 3-word prefix of IC fragment found in roon fragment
        if iw.len() >= 3 {
            let probe = format!("{} {} {}", iw[0], iw[1], iw[2]);
            if probe.len() >= 6 && roon_frag.contains(&probe) {
                best = best.max(3);
                continue;
            }
        }

        // 2-word consecutive pairs from roon (handles first-word spelling variants)
        let mut found = false;
        for window in rw.windows(2) {
            let probe = format!("{} {}", window[0], window[1]);
            if probe.len() >= 6 && ic_frag.contains(&probe) {
                best = best.max(2);
                found = true;
                break;
            }
        }
        if found { continue; }

        // 2-word consecutive pairs from IC
        for window in iw.windows(2) {
            let probe = format!("{} {}", window[0], window[1]);
            if probe.len() >= 6 && roon_frag.contains(&probe) {
                best = best.max(2);
                break;
            }
        }
    }

    best
}

/// Compare two ParsedTitles and return a match score.
/// Returns None if no match, Some(score) if matched. Higher score = better match.
/// Currently only implements text fragment comparison.
fn parsed_titles_match(roon: &ParsedTitle, ic: &ParsedTitle) -> Option<u32> {
    // Special case: overture/sinfonia synonyms
    let overture_words = ["sinfonia", "overture", "ouverture", "ouvertura", "preludio"];
    let roon_is_overture = roon.text_fragments.iter()
        .any(|f| overture_words.iter().any(|w| f.contains(w)));
    let ic_is_overture = ic.text_fragments.iter()
        .any(|f| overture_words.iter().any(|w| f.contains(w)));
    if roon_is_overture && ic_is_overture {
        return Some(10);
    }

    // Score each roon fragment against IC fragments
    let mut total_score = 0u32;
    for rf in &roon.text_fragments {
        total_score += best_fragment_score(rf, &ic.text_fragments);
    }

    if total_score > 0 { Some(total_score) } else { None }
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

    #[test]
    fn test_parse_roon_title_basic() {
        let p = parse_roon_title("Le nozze di Figaro, K. 492, Act I: No. 1, Duet. Cinque, dieci, venti \u{2013} Cosa stai misurando");
        assert_eq!(p.act, Some("1".to_string()));
        assert_eq!(p.number, Some(1));
        assert_eq!(p.type_label, Some("duet".to_string()));
        assert!(p.text_fragments.len() >= 2);
        assert!(p.text_fragments[0].contains("cinque"));
        assert!(p.text_fragments[1].contains("cosa stai"));
    }

    #[test]
    fn test_parse_roon_title_recit_prefix() {
        let p = parse_roon_title("Le nozze di Figaro, K. 492, Act I: Recit. Bravo, signor padrone \u{2013} No. 3, Se vuol ballare \u{2013} Ed aspettaste");
        assert_eq!(p.act, Some("1".to_string()));
        assert_eq!(p.number, Some(3));
        assert_eq!(p.type_label, Some("recitativo".to_string()));
        assert!(p.text_fragments.len() >= 3);
    }

    #[test]
    fn test_parse_roon_title_finale() {
        let p = parse_roon_title("Le nozze di Figaro, K. 492, Act II: No. 16, Finale: a-c. Esci omai garzon malnato ... Susanna, son morta");
        assert_eq!(p.act, Some("2".to_string()));
        assert_eq!(p.number, Some(16));
        assert_eq!(p.type_label, Some("finale".to_string()));
        assert!(!p.text_fragments.is_empty());
        assert!(p.text_fragments[0].contains("esci"));
        // Part label "a-c." should be stripped
        assert!(!p.text_fragments[0].starts_with("a"));
    }

    #[test]
    fn test_parse_roon_title_overture() {
        let p = parse_roon_title("Le nozze di Figaro, K. 492: Overture");
        assert_eq!(p.act, None);
        assert_eq!(p.number, None);
        assert!(p.text_fragments.iter().any(|f| f.contains("overture")));
    }

    #[test]
    fn test_parse_ic_title_basic() {
        let p = parse_ic_title(r#"No. 1 Duetto "Cinque...dieci...venti" (Figaro, Susanna); recitativo "Cosa stai misurando" (Susanna, Figaro)"#);
        assert_eq!(p.number, Some(1));
        assert_eq!(p.type_label, Some("duet".to_string()));
        assert!(p.text_fragments.len() >= 2);
        assert!(p.text_fragments[0].contains("cinque"));
        assert!(p.text_fragments[1].contains("cosa stai"));
    }

    #[test]
    fn test_parse_ic_title_finale() {
        let p = parse_ic_title(r#"No. 15 Finale, Part 1: "Esci, ormai, garzon malnato" (Conte, Contessa, Susanna)"#);
        assert_eq!(p.number, Some(15));
        assert_eq!(p.type_label, Some("finale".to_string()));
        assert_eq!(p.text_fragments.len(), 1);
        assert!(p.text_fragments[0].contains("esci"));
    }

    #[test]
    fn test_parse_ic_title_sinfonia() {
        let p = parse_ic_title("Sinfonia");
        assert_eq!(p.number, None);
        assert!(p.text_fragments.iter().any(|f| f.contains("sinfonia")));
    }

    #[test]
    fn test_parsed_match_overture_sinfonia() {
        let roon = parse_roon_title("Le nozze di Figaro, K. 492: Overture");
        let ic = parse_ic_title("Sinfonia");
        assert!(parsed_titles_match(&roon, &ic).is_some());
    }

    #[test]
    fn test_parsed_match_text_fragments() {
        // Track 2: exact 3-word match
        let roon = parse_roon_title("Le nozze di Figaro, K. 492, Act I: No. 1, Duet. Cinque, dieci, venti \u{2013} Cosa stai misurando");
        let ic = parse_ic_title(r#"No. 1 Duetto "Cinque...dieci...venti" (Figaro, Susanna); recitativo "Cosa stai misurando" (Susanna, Figaro)"#);
        let score = parsed_titles_match(&roon, &ic);
        assert!(score.is_some());
        assert!(score.unwrap() >= 3, "Expected score >= 3, got {:?}", score);
    }

    #[test]
    fn test_parsed_match_number_offset() {
        // Track 9: Roon No. 10 vs IC No. 9 — should match on text "non più andrai"
        let roon = parse_roon_title("Le nozze di Figaro, K. 492, Act I: No. 10, Non più andrai");
        let ic = parse_ic_title(r#"No. 9 Aria "Non più andrai" (Figaro)"#);
        assert!(parsed_titles_match(&roon, &ic).is_some());
    }

    #[test]
    fn test_parsed_match_spelling_variant_omai() {
        // Track 16: "Esci omai" vs "Esci, ormai" — 2-word pair "garzon malnato" matches
        let roon = parse_roon_title("Le nozze di Figaro, K. 492, Act II: No. 16, Finale: a-c. Esci omai garzon malnato ... Susanna, son morta");
        let ic = parse_ic_title(r#"No. 15 Finale, Part 1: "Esci, ormai, garzon malnato" (Conte, Contessa, Susanna)"#);
        let score = parsed_titles_match(&roon, &ic);
        assert!(score.is_some(), "Expected match for omai/ormai variant, got None");
    }

    #[test]
    fn test_parsed_match_spelling_variant_signore() {
        // Track 17: "Signore di fuori" vs "Signori, di fuori" — 2-word pair "di fuori" matches
        let roon = parse_roon_title("Le nozze di Figaro, K. 492, Act II: No. 16, Finale: d-g. Signore di fuori... Ah, signor, signor");
        let ic = parse_ic_title(r#"No. 15 Finale, Part 2: "Signori, di fuori son già" (Figaro, Conte, Contessa, Susanna, Antonio)"#);
        let score = parsed_titles_match(&roon, &ic);
        assert!(score.is_some(), "Expected match for Signore/Signori variant, got None");
    }

    #[test]
    fn test_parsed_match_recit_only() {
        // Track 13: "Recit. Che novità!" vs "Recitativo 'Che novità!'"
        let roon = parse_roon_title("Le nozze di Figaro, K. 492, Act II: Recit. Che novità!");
        let ic = parse_ic_title(r#"Recitativo "Che novità!" (Conte, Contessa)"#);
        assert!(parsed_titles_match(&roon, &ic).is_some());
    }

    #[test]
    fn test_parsed_match_no_false_positive() {
        // Track 16 Roon should NOT match IC track for Act III (different text)
        let roon = parse_roon_title("Le nozze di Figaro, K. 492, Act II: No. 16, Finale: a-c. Esci omai garzon malnato ... Susanna, son morta");
        let ic = parse_ic_title(r#"No. 16 Duetto "Crudel! perchè finora" (Conte, Susanna); recitativo "E perchè fosti meco" (Conte, Susanna, Figaro)"#);
        // This should NOT match — completely different text
        let score = parsed_titles_match(&roon, &ic);
        assert!(score.is_none(), "Expected no match for different text, got {:?}", score);
    }

    /// Test all 32 Figaro track pairs from the Solti box set to ensure correct matching.
    #[test]
    fn test_all_32_figaro_pairs() {
        let roon_titles = vec![
            "Le nozze di Figaro, K. 492: Overture",
            "Le nozze di Figaro, K. 492, Act I: No. 1, Duet. Cinque, dieci, venti \u{2013} Cosa stai misurando",
            "Le nozze di Figaro, K. 492, Act I: No. 2, Duet. Se a caso madama \u{2013} Or bene, ascolta, e taci",
            "Le nozze di Figaro, K. 492, Act I: Recit. Bravo, signor padrone \u{2013} No. 3, Se vuol ballare \u{2013} Ed aspettaste",
            "Le nozze di Figaro, K. 492, Act I: No. 4, La vendetta \u{2013} Tutto ancor non ho perso",
            "Le nozze di Figaro, K. 492, Act I: No. 5, Duet. Via resti servita \u{2013} Va' l\u{e0}, vecchia pedante",
            "Le nozze di Figaro, K. 492, Act I: No. 6, Non so pi\u{f9} \u{2013} Ah, son perduto!",
            "Le nozze di Figaro, K. 492, Act I: No. 7, Trio. Cosa sento! \u{2013} Basilio, in traccia tosto \u{2013} No. 8, Chorus. Giovani liete",
            "Le nozze di Figaro, K. 492, Act I: No. 10, Non pi\u{f9} andrai",
            "Le nozze di Figaro, K. 492, Act II: No. 11, Porgi amor \u{2013} Vieni, cara Susanna \u{2013} Quanto duolmi, Susanna",
            "Le nozze di Figaro, K. 492, Act II: No. 12, Voi che sapete \u{2013} Bravo! che bella voce!",
            "Le nozze di Figaro, K. 492, Act II: No. 13, Venite inginocchiatevi \u{2013} Quante buffonerie!",
            "Le nozze di Figaro, K. 492, Act II: Recit. Che novit\u{e0}!",
            "Le nozze di Figaro, K. 492, Act II: No. 14, Trio. Susanna or via sortite \u{2013} Dunque, voi non aprite",
            "Le nozze di Figaro, K. 492, Act II: No. 15, Aprite presto aprite \u{2013} O guarda \u{2013} Tutto \u{e8} come io lasciai",
            "Le nozze di Figaro, K. 492, Act II: No. 16, Finale: a-c. Esci omai garzon malnato ... Susanna, son morta",
            "Le nozze di Figaro, K. 492, Act II: No. 16, Finale: d-g. Signore di fuori... Ah, signor, signor",
            "Le nozze di Figaro, K. 492, Act II: No. 16, Finale: h-i. Voi signor, che giusto siete",
            "Le nozze di Figaro, K. 492, Act III: Recit. Che imbarazzo \u{e8} mai questo \u{2013} Via, fatti core",
            "Le nozze di Figaro, K. 492, Act III: No. 17, Duet. Crudel! perch\u{e8} finora \u{2013} Recit. E perch\u{e9} fosti meco",
            "Le nozze di Figaro, K. 492, Act III: Recit. Hai gi\u{e0} vinta la causa! \u{2013} No. 18, Vedro mentr'io sospiro \u{2013} Recit. E decisa la lite",
            "Le nozze di Figaro, K. 492, Act III: No. 19, Sextet. Riconosci in questo amplesso \u{2013} Recit. Eccovi, o caro amico \u{2013} Andiam, bel paggio",
            "Le nozze di Figaro, K. 492, Act III: No. 20, E Susanna non vien! \u{2013} Dove sono i bei momenti \u{2013} Recit. Io vi dico, signor",
            "Le nozze di Figaro, K. 492, Act III: Recit. Cosa mi narri \u{2013} No. 21, Sull' aria... Che soave zeffiretto \u{2013} Recit. Piegato \u{e8} il foglio \u{2013} No. 22, Chorus. Ricevete \u{2013} Recit. Queste sono",
            "Le nozze di Figaro, K. 492, Act III: No. 23, Finale: a-e. Ecco la marcia, andiamo... Andate amici!",
            "Le nozze di Figaro, K. 492, Act IV: No. 24, L'ho perduta, me meschina \u{2013} Recit. Barbarina, cos'hai?... Presto, avvertiam Susanna",
            "Le nozze di Figaro, K. 492, Act IV: No. 25, Il capro e la capretta \u{2013} Recit. Nel padiglione a manca... Ha i diavoli nel corpo",
            "Le nozze di Figaro, K. 492, Act IV: No. 26, In quegli anni in cui val poco",
            "Le nozze di Figaro, K. 492, Act IV: Recit. Tutto \u{e8} disposto \u{2013} No. 27, Aprite un po' quegl' occhi",
            "Le nozze di Figaro, K. 492, Act IV: No. 28, Giunse alfin il momento \u{2013} Deh vieni, non tardar",
            "Le nozze di Figaro, K. 492, Act IV: No. 29, Finale: a-d. Pian pianin le andr\u{f2} pi\u{f9} presso... Pace, pace, mio dolce tesoro",
            "Le nozze di Figaro, K. 492, Act IV: No. 29, Finale: e-g. Gente, gente, all'armi... Questo giorno di tormenti",
        ];

        let ic_titles = vec![
            "Sinfonia",
            r#"No. 1 Duetto "Cinque...dieci...venti" (Figaro, Susanna); recitativo "Cosa stai misurando" (Susanna, Figaro)"#,
            r#"No. 2 Duetto "Se a caso madama" (Figaro, Susanna); recitativo "Or bene, ascolta, e taci" (Susanna, Figaro)"#,
            r#"Recitativo "Bravo, signor padrone" (Figaro); No. 3 Cavatina "Se vuol ballare" (Figaro); recitativo "Ed aspettaste il giorno" (Bartolo, Marcellina)"#,
            r#"No. 4 Aria "La vendetta" (Bartolo); recitativo "Tutto ancor non ho perso" (Marcellina, Susanna)"#,
            r#"No. 5 Duetto "Via resti servita" (Marcellina, Susanna); recitativo "Va là, vecchia pedante" (Susanna)"#,
            r#"No. 6 Aria "Non so più" (Cherubino); recitativo "Ah, son perduto" (Cherubino, Susanna, Conte)"#,
            r#"No. 7 Terzetto "Cosa sento!" (Conte, Basilio, Susanna); recitativo "Basilio, in traccia tosto" (Conte); No. 8a Coro "Giovanni liete" (Coro, Figaro, Susanna, Conte)"#,
            r#"No. 9 Aria "Non più andrai" (Figaro)"#,
            r#"No. 10 Cavatina "Porgi, amor, qualche ristoro" (Contessa); recitativo "Vieni, cara Susanna" (Contessa, Susanna)"#,
            r#"No. 11 Canzona "Voi che sapete" (Cherubino); recitativo "Bravo! che bella voce" (Contessa, Susanna, Cherubino)"#,
            r#"No. 12 Aria "Venite, inginocchiatevi" (Susanna); recitativo "Quante buffonerie" (Contessa, Susanna)"#,
            r#"Recitativo "Che novità!" (Conte, Contessa)"#,
            r#"No. 13 Terzetto "Susanna or via sortite" (Conte, Contessa, Susanna); recitativo "Dunque, voi non aprite" (Conte, Contessa)"#,
            r#"No. 14 Duettino "Aprite, presto, aprite" (Susanna, Cherubino); recitativo "O guarda il demonietto" (Susanna, Conte, Contessa)"#,
            r#"No. 15 Finale, Part 1: "Esci, ormai, garzon malnato" (Conte, Contessa, Susanna)"#,
            r#"No. 15 Finale, Part 2: "Signori, di fuori son già" (Figaro, Conte, Contessa, Susanna, Antonio)"#,
            r#"No. 15 Finale, Part 3: "Voi signor, che giusto siete" (Marcellina, Basilio, Bartolo, Conte, Contessa, Susanna, Figaro)"#,
            r#"Recitativo "Che imbarazzo è mai questo" (Conte, Contessa, Susanna)"#,
            r#"No. 16 Duetto "Crudel! perchè finora" (Conte, Susanna); recitativo "E perchè fosti meco" (Conte, Susanna, Figaro)"#,
            r#"No. 17 Recitativo "Hai già vinta la causa?" ed Aria "Vedrò, mentr'io sospiro" (Conte); recitativo "È decisa la lite" (Marcellina, Curzio, Conte, Bartolo, Figaro)"#,
            r#"No. 18 Sestetto "Riconosci in questo amplesso" (Marcellina, Figaro, Bartolo, Curzio, Conte, Susanna); recitativo "Eccovi, o caro amico" (Marcellina, Bartolo, Susanna, Figaro)"#,
            r#"No. 19 Recitativo "E Susanna non vien" ed Aria "Dove sono i bei momenti" (Contessa); recitativo "Io vi dico, signor" (Antonio, Conte)"#,
            r#"Recitativo "Cosa mi narri" (Contessa, Susanna); No. 20 Duetto "Sull'aria ... che soave zeffiretto" (Contessa, Susanna); recitativo "Piegato è il foglio" (Susanna); No. 21 Coro "Ricevete, o padroncina" (Coro); recitativo "Queste sono, Madama" (Barbarina, Contessa, Susanna, Antonio, Conte)"#,
            r#"No. 22 Finale: "Ecco la marcia" (Figaro, Susanna, Conte, Contessa, Chorus)"#,
            r#"No. 23 Cavatina " L'ho perduta, me meschina" (Barbarina); recitativo "Barbarina, cos'hai" (Figaro, Barbarina, Marcellina)"#,
            r#"No. 24 Aria "Il capro e la capretta" (Marcellina); recitativo "Nel padiglione a manca" (Barbarina, Figaro, Basilio, Bartolo)"#,
            r#"No. 25 Aria "In quegli anni in cui val poco" (Basilio)"#,
            r#"No. 26 Recitativo "Tutto è disposto" ed Aria "O Aprite un po' quegli occhi" (Figaro); recitativo "Signora, ella mi disse" (Susanna, Marcellina, Contessa, Figaro)"#,
            r#"No. 27 Recitativo "Giunse alfin il momento" ed Aria "Deh vieni, non tardar" (Susanna); recitativo "Perfida! E in quella forma" (Figaro, Cherubino, Contessa)"#,
            r#"No. 28 Finale, Part 1: "Pian pianin le andrò più presso" (Cherubino, Contessa, Conte, Susanna, Figaro)"#,
            r#"No. 28 Finale, Part 2: " Gente! gente! all'armi! all'armi!" (Conte, tutti)"#,
        ];

        assert_eq!(roon_titles.len(), 32);
        assert_eq!(ic_titles.len(), 32);

        let mut failures = Vec::new();
        for (i, (roon_raw, ic_raw)) in roon_titles.iter().zip(ic_titles.iter()).enumerate() {
            let roon = parse_roon_title(roon_raw);
            let ic = parse_ic_title(ic_raw);
            let score = parsed_titles_match(&roon, &ic);

            if score.is_none() {
                failures.push(format!("Track {}: no match\n  Roon: {}\n  IC:   {}\n  Roon frags: {:?}\n  IC frags:   {:?}",
                    i + 1, roon_raw, ic_raw, roon.text_fragments, ic.text_fragments));
            }

            // Also verify it picks the RIGHT track (not a different one) by checking
            // that no other IC track scores higher
            if let Some(correct_score) = score {
                for (j, other_ic_raw) in ic_titles.iter().enumerate() {
                    if j == i { continue; }
                    let other_ic = parse_ic_title(other_ic_raw);
                    if let Some(other_score) = parsed_titles_match(&roon, &other_ic) {
                        if other_score > correct_score {
                            failures.push(format!("Track {}: wrong target! Correct score={} but track {} scores {}",
                                i + 1, correct_score, j + 1, other_score));
                        }
                    }
                }
            }
        }

        if !failures.is_empty() {
            panic!("Figaro matching failures:\n{}", failures.join("\n\n"));
        }
    }
}
