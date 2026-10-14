//! Local persistence for dictation history and snippets.
//!
//! One JSON file under `%APPDATA%\UnoFlow\`. Deliberately simple: a plain list
//! we rewrite on change. The volume here is tiny (a few thousand short strings),
//! so a database would be overkill.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Unix epoch milliseconds.
    pub ts: u64,
    pub text: String,
    pub duration_ms: u64,
    /// Process name of the app that had focus (e.g. `chrome.exe`).
    #[serde(default)]
    pub app: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub trigger: String,
    pub expansion: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub history: Vec<Entry>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    /// Custom words/phrases fed to the model as a prompt.
    #[serde(default)]
    pub dictionary: Vec<String>,
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Serialize)]
pub struct Stats {
    pub total_words: u64,
    pub wpm: u64,
    pub day_streak: u64,
    pub total_entries: u64,
}

/// One bar in the daily-activity chart.
#[derive(Serialize)]
pub struct DayCount {
    /// Epoch day number (ts / 86_400_000).
    pub day: u64,
    pub words: u64,
    pub entries: u64,
}

/// One row in the "apps you dictate into" list.
#[derive(Serialize)]
pub struct AppCount {
    pub app: String,
    pub words: u64,
    pub entries: u64,
}

#[derive(Serialize)]
pub struct Insights {
    pub total_words: u64,
    pub total_entries: u64,
    pub wpm: u64,
    pub day_streak: u64,
    pub avg_words: u64,
    pub total_minutes: u64,
    /// Last 14 days, oldest first.
    pub daily: Vec<DayCount>,
    pub apps: Vec<AppCount>,
    /// Most-used words, lowercase, punctuation stripped, length >= 4.
    pub top_words: Vec<(String, u64)>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Store {
    pub fn load(path: PathBuf) -> Self {
        let mut s: Store = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.path = path;
        s
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(self).unwrap())
    }

    /// Prepend a new transcript. Newest first, which is how the UI reads it.
    pub fn add(&mut self, text: String, duration_ms: u64, app: String) -> std::io::Result<()> {
        let ts = now_ms();
        let id = format!("{ts}-{}", self.history.len());
        self.history.insert(
            0,
            Entry {
                id,
                ts,
                text,
                duration_ms,
                app,
            },
        );
        self.history.truncate(2000);
        self.save()
    }

    pub fn delete(&mut self, id: &str) -> std::io::Result<()> {
        self.history.retain(|e| e.id != id);
        self.save()
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.history.clear();
        self.save()
    }

    pub fn set_snippets(&mut self, snippets: Vec<Snippet>) -> std::io::Result<()> {
        self.snippets = snippets;
        self.save()
    }

    pub fn set_dictionary(&mut self, words: Vec<String>) -> std::io::Result<()> {
        self.dictionary = words;
        self.save()
    }

    pub fn stats(&self) -> Stats {
        let total_words: u64 = self
            .history
            .iter()
            .map(|e| e.text.split_whitespace().count() as u64)
            .sum();
        let total_ms: u64 = self.history.iter().map(|e| e.duration_ms).sum();
        let wpm = if total_ms > 0 {
            total_words * 60_000 / total_ms
        } else {
            0
        };

        // Length of the run of consecutive days ending at the most recent day
        // that has a transcript.
        let mut days: Vec<u64> = self.history.iter().map(|e| e.ts / 86_400_000).collect();
        days.sort_unstable();
        days.dedup();
        let mut streak = 0u64;
        if let Some(&last) = days.last() {
            let mut expected = last;
            for &d in days.iter().rev() {
                if d == expected {
                    streak += 1;
                    expected = expected.saturating_sub(1);
                } else if d < expected {
                    break;
                }
            }
        }

        Stats {
            total_words,
            wpm,
            day_streak: streak,
            total_entries: self.history.len() as u64,
        }
    }

    pub fn insights(&self) -> Insights {
        use std::collections::HashMap;

        let total_words: u64 = self
            .history
            .iter()
            .map(|e| e.text.split_whitespace().count() as u64)
            .sum();
        let total_entries = self.history.len() as u64;
        let total_ms: u64 = self.history.iter().map(|e| e.duration_ms).sum();
        let wpm = if total_ms > 0 {
            total_words * 60_000 / total_ms
        } else {
            0
        };
        let avg_words = if total_entries > 0 {
            total_words / total_entries
        } else {
            0
        };

        // Daily buckets.
        let mut by_day: HashMap<u64, (u64, u64)> = HashMap::new();
        for e in &self.history {
            let d = e.ts / 86_400_000;
            let slot = by_day.entry(d).or_insert((0, 0));
            slot.0 += e.text.split_whitespace().count() as u64;
            slot.1 += 1;
        }
        let today = now_ms() / 86_400_000;
        let mut daily = Vec::with_capacity(14);
        for back in (0..14).rev() {
            let day = today.saturating_sub(back);
            let (words, entries) = by_day.get(&day).copied().unwrap_or((0, 0));
            daily.push(DayCount { day, words, entries });
        }

        // Apps.
        let mut by_app: HashMap<String, (u64, u64)> = HashMap::new();
        for e in &self.history {
            let key = if e.app.is_empty() {
                "unknown".to_string()
            } else {
                e.app.clone()
            };
            let slot = by_app.entry(key).or_insert((0, 0));
            slot.0 += e.text.split_whitespace().count() as u64;
            slot.1 += 1;
        }
        let mut apps: Vec<AppCount> = by_app
            .into_iter()
            .map(|(app, (words, entries))| AppCount { app, words, entries })
            .collect();
        apps.sort_by(|a, b| b.words.cmp(&a.words));
        apps.truncate(8);

        // Top words.
        let mut words: HashMap<String, u64> = HashMap::new();
        for e in &self.history {
            for raw in e.text.split_whitespace() {
                let w: String = raw
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '\'')
                    .collect::<String>()
                    .to_lowercase();
                if w.len() >= 4 {
                    *words.entry(w).or_insert(0) += 1;
                }
            }
        }
        let mut top_words: Vec<(String, u64)> = words.into_iter().collect();
        top_words.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top_words.truncate(12);

        Insights {
            total_words,
            total_entries,
            wpm,
            day_streak: self.stats().day_streak,
            avg_words,
            total_minutes: total_ms / 60_000,
            daily,
            apps,
            top_words,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(entries: Vec<Entry>) -> Store {
        Store {
            history: entries,
            snippets: vec![],
            dictionary: vec![],
            path: PathBuf::new(),
        }
    }

    fn entry(ts: u64, text: &str, duration_ms: u64, app: &str) -> Entry {
        Entry {
            id: format!("{ts}"),
            ts,
            text: text.to_string(),
            duration_ms,
            app: app.to_string(),
        }
    }

    #[test]
    fn stats_counts_words_and_wpm() {
        // 4 + 2 = 6 words over 60s total => 6 wpm.
        let s = store_with(vec![
            entry(1_000, "one two three four", 30_000, "chrome.exe"),
            entry(2_000, "five six", 30_000, "chrome.exe"),
        ]);
        let st = s.stats();
        assert_eq!(st.total_words, 6);
        assert_eq!(st.total_entries, 2);
        assert_eq!(st.wpm, 6);
    }

    #[test]
    fn stats_streak_counts_consecutive_days() {
        let day = 86_400_000u64;
        let today = now_ms() / day;
        // Today and yesterday => streak of 2.
        let s = store_with(vec![
            entry(today * day, "a", 1000, "x"),
            entry((today - 1) * day, "b", 1000, "x"),
        ]);
        assert_eq!(s.stats().day_streak, 2);

        // A gap (today and 3 days ago) => streak of 1.
        let s2 = store_with(vec![
            entry(today * day, "a", 1000, "x"),
            entry((today - 3) * day, "b", 1000, "x"),
        ]);
        assert_eq!(s2.stats().day_streak, 1);
    }

    #[test]
    fn insights_groups_by_app_and_word() {
        let s = store_with(vec![
            entry(1_000, "hello world hello", 1000, "chrome.exe"),
            entry(2_000, "hello there", 1000, "notepad.exe"),
            entry(3_000, "hello again", 1000, "chrome.exe"),
        ]);
        let d = s.insights();

        assert_eq!(d.total_entries, 3);
        assert_eq!(d.total_words, 7);

        // chrome has 2 entries / 5 words, so it sorts first.
        assert_eq!(d.apps[0].app, "chrome.exe");
        assert_eq!(d.apps[0].words, 5);
        assert_eq!(d.apps[1].app, "notepad.exe");

        // "hello" appears 4 times and is the top word.
        assert_eq!(d.top_words[0], ("hello".to_string(), 4));

        // 14 daily buckets, oldest first.
        assert_eq!(d.daily.len(), 14);
    }

    #[test]
    fn insights_survives_empty_history() {
        let d = store_with(vec![]).insights();
        assert_eq!(d.total_words, 0);
        assert_eq!(d.wpm, 0);
        assert_eq!(d.avg_words, 0);
        assert_eq!(d.daily.len(), 14);
        assert!(d.apps.is_empty());
        assert!(d.top_words.is_empty());
    }
}
