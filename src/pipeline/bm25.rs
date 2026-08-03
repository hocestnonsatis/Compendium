//! Lightweight BM25 lexical scoring for query-aware keep / rerank.
//!
//! Deterministic, zero-GPU. Tuned to preserve technical identifiers (versions,
//! status codes, UUIDs, hex ids) that pure semantic summarizers often drop.

use std::collections::HashMap;

use regex::Regex;
use std::sync::OnceLock;

/// BM25 parameters (classic Okapi defaults).
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Tokenize for BM25: alphanumerics plus `_` `-` `.` `/` `#`.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '#')))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 2)
        .filter(|s| !is_stopword(s))
        .collect()
}

fn is_stopword(s: &str) -> bool {
    matches!(
        s,
        "the"
            | "and"
            | "for"
            | "with"
            | "this"
            | "that"
            | "from"
            | "into"
            | "your"
            | "have"
            | "been"
            | "were"
            | "was"
            | "are"
            | "will"
            | "can"
            | "may"
            | "not"
            | "but"
            | "all"
            | "any"
            | "out"
            | "use"
            | "using"
    )
}

/// True when a token looks like an ID, version, status code, or path fragment.
pub fn is_technical_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    // HTTP / process status
    if matches!(
        token,
        "200" | "201" | "204" | "301" | "302" | "400" | "401" | "403" | "404" | "409" | "422"
            | "429" | "500" | "502" | "503" | "504"
    ) {
        return true;
    }
    // Semver-ish: 1.2.3 / v1.2.3-beta
    if SEMVER.get_or_init(|| Regex::new(r"^v?\d+\.\d+(\.\d+)?([-+][a-z0-9.]+)?").expect("re"))
        .is_match(token)
    {
        return true;
    }
    // Hex id / commit-ish (7–40 hex)
    if HEX_ID
        .get_or_init(|| Regex::new(r"^[0-9a-f]{7,40}$").expect("re"))
        .is_match(token)
    {
        return true;
    }
    // UUID
    if UUID
        .get_or_init(|| {
            Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
                .expect("re")
        })
        .is_match(token)
    {
        return true;
    }
    // Path-like or namespaced id
    if token.contains('/') || token.contains("::") || token.contains('#') {
        return true;
    }
    // ERROR/WARN/FAIL markers treated as technical signal words
    matches!(
        token,
        "error" | "err" | "warn" | "warning" | "fail" | "failed" | "failure" | "panic" | "fatal"
            | "exception" | "traceback" | "errno"
    )
}

static SEMVER: OnceLock<Regex> = OnceLock::new();
static HEX_ID: OnceLock<Regex> = OnceLock::new();
static UUID: OnceLock<Regex> = OnceLock::new();

/// Score each document against `query` with BM25. Returns `(index, score)` sorted desc.
pub fn score_documents(query: &str, docs: &[&str]) -> Vec<(usize, f64)> {
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() || docs.is_empty() {
        return docs.iter().enumerate().map(|(i, _)| (i, 0.0)).collect();
    }

    let doc_tokens: Vec<Vec<String>> = docs.iter().map(|d| tokenize(d)).collect();
    let n = doc_tokens.len() as f64;
    let avgdl = if n == 0.0 {
        0.0
    } else {
        doc_tokens.iter().map(|t| t.len() as f64).sum::<f64>() / n
    };

    let mut df: HashMap<&str, usize> = HashMap::new();
    for tokens in &doc_tokens {
        let mut seen = std::collections::HashSet::new();
        for t in tokens {
            if seen.insert(t.as_str()) {
                *df.entry(t.as_str()).or_insert(0) += 1;
            }
        }
    }

    let mut scores: Vec<(usize, f64)> = doc_tokens
        .iter()
        .enumerate()
        .map(|(idx, tokens)| {
            let mut score = bm25_doc(&query_tokens, tokens, &df, n, avgdl);
            // Preserve technical identifiers mentioned in the query or present as high-signal.
            let tech_boost = tokens
                .iter()
                .filter(|t| is_technical_token(t))
                .filter(|t| {
                    query_tokens.iter().any(|q| q == *t || t.contains(q.as_str()) || q.contains(t.as_str()))
                        || is_technical_token(t) && query_tokens.iter().any(|q| is_technical_token(q))
                })
                .count();
            // Always nudge lines that carry tech tokens when the query also has tech tokens,
            // and boost exact technical overlaps more strongly.
            let exact_tech = tokens
                .iter()
                .filter(|t| is_technical_token(t) && query_tokens.iter().any(|q| q == *t))
                .count();
            score += exact_tech as f64 * 1.5;
            if exact_tech == 0 && tech_boost > 0 {
                score += 0.15 * tech_boost.min(3) as f64;
            }
            // Error/warn lines with any query overlap
            let upper = docs[idx].to_ascii_uppercase();
            if score > 0.0
                && (upper.contains("ERROR")
                    || upper.contains("WARN")
                    || upper.contains("FAIL")
                    || upper.contains("PANIC"))
            {
                score += 0.35;
            }
            (idx, score)
        })
        .collect();

    scores.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scores
}

fn bm25_doc(
    query_tokens: &[String],
    doc_tokens: &[String],
    df: &HashMap<&str, usize>,
    n_docs: f64,
    avgdl: f64,
) -> f64 {
    if doc_tokens.is_empty() || avgdl <= 0.0 {
        return 0.0;
    }
    let mut tf: HashMap<&str, usize> = HashMap::new();
    for t in doc_tokens {
        *tf.entry(t.as_str()).or_insert(0) += 1;
    }
    let dl = doc_tokens.len() as f64;
    let mut score = 0.0;
    let mut seen_q = std::collections::HashSet::new();
    for q in query_tokens {
        if !seen_q.insert(q.as_str()) {
            continue; // unique query terms
        }
        let f = *tf.get(q.as_str()).unwrap_or(&0) as f64;
        if f == 0.0 {
            // soft substring match for technical fragments
            let soft = doc_tokens
                .iter()
                .filter(|t| t.contains(q.as_str()) || q.contains(t.as_str()))
                .count() as f64;
            if soft == 0.0 {
                continue;
            }
            let idf = idf(df.get(q.as_str()).copied().unwrap_or(1), n_docs);
            score += idf * (soft * (K1 + 1.0)) / (soft + K1 * (1.0 - B + B * dl / avgdl)) * 0.5;
            continue;
        }
        let n_q = df.get(q.as_str()).copied().unwrap_or(0);
        let idf = idf(n_q, n_docs);
        score += idf * (f * (K1 + 1.0)) / (f + K1 * (1.0 - B + B * dl / avgdl));
    }
    score
}

fn idf(df: usize, n_docs: f64) -> f64 {
    let n_q = df as f64;
    ((n_docs - n_q + 0.5) / (n_q + 0.5) + 1.0).ln()
}

/// Keep lines relevant to `query` via BM25, preserving original order.
pub fn filter_lines_bm25(
    input: &str,
    query: &str,
    max_tokens: usize,
    estimate_tokens: impl Fn(&str) -> usize,
) -> String {
    let lines: Vec<&str> = input
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }

    let scored = score_documents(query, &lines);
    let positive: Vec<(usize, f64)> = scored
        .iter()
        .copied()
        .filter(|(_, s)| *s > 0.0)
        .collect();

    let chosen: Vec<(usize, f64)> = if positive.is_empty() {
        scored.into_iter().take(8).collect()
    } else {
        positive
    };

    let mut indices: Vec<usize> = chosen.iter().map(|(i, _)| *i).collect();
    indices.sort_unstable();
    indices.dedup();

    // Always keep lines that contain technical tokens also present in the query,
    // even if BM25 scored them low (metadata preservation).
    let q_tokens = tokenize(query);
    let q_tech: Vec<&String> = q_tokens.iter().filter(|t| is_technical_token(t)).collect();
    if !q_tech.is_empty() {
        for (idx, line) in lines.iter().enumerate() {
            let lt = tokenize(line);
            if lt.iter().any(|t| q_tech.iter().any(|qt| *qt == t)) && !indices.contains(&idx) {
                indices.push(idx);
            }
        }
        indices.sort_unstable();
    }

    let mut out = String::new();
    for idx in indices {
        let line = lines[idx];
        let candidate = if out.is_empty() {
            line.to_string()
        } else {
            format!("{out}\n{line}")
        };
        if estimate_tokens(&candidate) > max_tokens {
            break;
        }
        out = candidate;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_relevant_doc_higher() {
        let docs = [
            "INFO heartbeat ok",
            "ERROR auth token expired for user 42",
            "DEBUG spinner frame",
        ];
        let ranked = score_documents("auth token expired", &docs);
        assert_eq!(ranked[0].0, 1);
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn preserves_version_and_status() {
        let docs = [
            "deployed widget successfully",
            "upgrade failed: status 503 package foo@1.2.3",
            "unrelated changelog entry",
        ];
        let ranked = score_documents("foo 1.2.3 status 503", &docs);
        assert_eq!(ranked[0].0, 1);
    }

    #[test]
    fn filter_keeps_tech_line() {
        let input = "noise\nFAIL pkg@2.0.1 errno 404\nmore noise\n";
        let out = filter_lines_bm25(input, "pkg 2.0.1 404", 256, |s| s.len() / 4);
        assert!(out.contains("2.0.1"));
        assert!(out.contains("404"));
    }
}
