//! Shared native launch matching. Inputs remain transient source-backed reads.
use std::path::Path;

use crate::ToolCall;

/// Native launch locations retain at most this many calls, keeping the newest.
pub(crate) const MAX_CALLS: usize = 4096;
/// Bound candidate inputs as well as the number of retained locations.
pub(crate) const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const CLOCK_SKEW_MS: i64 = 2000;

pub(crate) struct Call {
    pub(crate) tool: ToolCall,
    pub(crate) time: Option<i64>,
    pub(crate) input: String,
}

pub(crate) struct Matcher<'a> {
    prompt: &'a str,
    prompt_name: Option<&'a str>,
    cwd: Option<&'a str>,
    start: Option<i64>,
    named: Vec<ToolCall>,
    fallback: Vec<(i64, ToolCall)>,
}

impl<'a> Matcher<'a> {
    pub(crate) fn new(prompt: &'a str, cwd: Option<&'a str>, start: Option<i64>) -> Self {
        Self {
            prompt,
            prompt_name: Path::new(prompt).file_name().and_then(|name| name.to_str()),
            cwd: cwd.filter(|cwd| {
                Path::new(cwd).components().count() >= 3
                    && std::env::var_os("HOME")
                        .is_none_or(|home| Path::new(cwd) != Path::new(&home))
            }),
            start,
            named: Vec::new(),
            fallback: Vec::new(),
        }
    }

    pub(crate) fn consider(&mut self, call: &Call) -> Option<ToolCall> {
        if call.input.contains(self.prompt) {
            return Some(call.tool.clone());
        }
        if self
            .prompt_name
            .is_some_and(|name| matches_segment(&call.input, name))
        {
            self.named.push(call.tool.clone());
        }
        if let (Some(cwd), Some(start), Some(time)) = (self.cwd, self.start, call.time)
            && time <= start.saturating_add(CLOCK_SKEW_MS)
            && matches_cwd(&call.input, cwd)
        {
            self.fallback.push((time, call.tool.clone()));
        }
        None
    }

    pub(crate) fn finish(mut self) -> Option<ToolCall> {
        if self.named.len() == 1 {
            self.named.pop()
        } else if self.named.is_empty() {
            self.fallback.sort_by_key(|(time, _)| *time);
            let (time, call) = self.fallback.pop()?;
            if self
                .fallback
                .last()
                .is_some_and(|(other, _)| time.saturating_sub(*other) <= CLOCK_SKEW_MS)
            {
                None
            } else {
                Some(call)
            }
        } else {
            None
        }
    }
}

/// A shell path component ends at slash, whitespace or shell punctuation.
/// Dots, hyphens and underscores stay in the name, so `foo` cannot join `foobar`.
fn matches_segment(input: &str, needle: &str) -> bool {
    let boundary =
        |character: char| character.is_whitespace() || "/\\\"'`;|&(){}[]<>=$,:".contains(character);
    let contains = |needle: &str| {
        !needle.is_empty()
            && input.match_indices(needle).any(|(at, _)| {
                input[..at].chars().next_back().is_none_or(boundary)
                    && input[at + needle.len()..]
                        .chars()
                        .next()
                        .is_none_or(boundary)
            })
    };
    contains(needle)
}

fn matches_cwd(input: &str, cwd: &str) -> bool {
    let cwd = cwd.trim_end_matches('/');
    matches_segment(input, cwd)
        || cwd
            .rsplit('/')
            .next()
            .is_some_and(|name| matches_segment(input, name))
}

#[cfg(test)]
mod tests;
