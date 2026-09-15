use std::{ffi::OsStr, iter::once};

use clap_complete::CompletionCandidate;
use crabbyconsole_misc::util::{build_key_name_map, get_all_resources, get_root};

pub fn resource_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    rank_completions(current, get_all_resources().iter())
}

pub fn scene_file_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    let all_resources = get_all_resources();
    let scene_files = all_resources.iter().filter(|res| {
        let res_lower = res.to_lowercase();

        // TODO maybe also allow .glb and .blend and others?
        res_lower.ends_with(".tscn") || res_lower.ends_with(".gltf")
    });

    rank_completions(current, scene_files)
}

pub fn nodepath_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    let all_nodes = get_root().find_children_ex("*").owned(false).done();

    // Don't forget to prepend the root node to `all_nodes`:
    let paths = once(get_root().upcast())
        .chain(all_nodes.iter_shared())
        .map(|node| node.get_path().to_string());

    rank_completions(current, paths)
}

pub fn key_completer(current: &OsStr) -> Vec<CompletionCandidate> {
    let map = build_key_name_map();
    let keys = map.iter().map(|(k, _)| k).collect::<Vec<_>>();

    rank_completions(current, keys)
}

// Future ideas for completers:
// (note - these will need some kind of mechanism to access them globally -> get_autoload?)
// - watch names
// - set/get variable names
// - keys
// - key bind names

/// Filters `items` against `current`, ranking prefix matches ahead of substring
/// matches, and returns them as sorted completion candidates.
fn rank_completions<I>(current: &OsStr, items: I) -> Vec<CompletionCandidate>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let Some(current) = current.to_str() else {
        tracing::warn!("invalid UTF-8 in `current` string - returning empty completion list");
        return vec![];
    };
    let current = current.to_lowercase();

    let mut completions: Vec<(bool, CompletionCandidate)> = items
        .into_iter()
        .filter_map(|item| {
            let item = item.as_ref();
            let item_lower = item.to_lowercase();

            if item_lower.starts_with(&current) {
                Some((true, CompletionCandidate::new(item))) // perfect match
            } else if item_lower.contains(&current) {
                Some((false, CompletionCandidate::new(item))) // no perfect match
            } else {
                None
            }
        })
        .collect();

    completions.sort_by_key(|(perfect_match, candidate)| {
        (
            if *perfect_match { 0 } else { 1 },
            candidate.get_value().to_os_string(),
        )
    });

    completions.into_iter().map(|(_, v)| v).collect()
}
