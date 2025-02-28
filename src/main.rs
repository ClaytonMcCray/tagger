use anyhow::Context;
use clap::Parser;
use itertools::Itertools;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

static TAGGER_FILE_NAMES: Lazy<HashSet<&'static str>> =
    Lazy::new(|| HashSet::from([".tagger.yaml", "tagger.yaml"]));

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The directories to operate on.
    #[arg(required = true)]
    dirs: Vec<std::path::PathBuf>,

    /// Regular expressions representing the tags to match on.
    /// Leave out for interactive mode.
    #[arg(long, short)]
    tags: Option<Vec<String>>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let (interactive, tags) = match args.tags {
        Some(tags) => (false, tags),
        None => (true, interactive_get_tags()?),
    };

    let results = args
        .dirs
        .iter()
        .map(|path| path.canonicalize())
        .map_ok(|path| process_directory_tree(&path, &tags))
        .collect::<Result<anyhow::Result<Vec<TaggedFiles>>, _>>()??;

    let mut deduplicated = HashMap::new();
    for tagged_file in results.into_iter() {
        for (k, v) in tagged_file.0.into_iter() {
            deduplicated
                .entry(k)
                .and_modify(|existing: &mut HashSet<String>| existing.extend(v.clone()))
                .or_insert(HashSet::from_iter(v.into_iter()));
        }
    }

    println!("{}", serde_yaml::to_string(&deduplicated)?);

    if interactive {
        wait_for_input()?;
    }

    Ok(())
}

fn wait_for_input() -> Result<(), io::Error> {
    println!("\npress enter to quit");
    let mut input = String::default();
    io::stdin().read_line(&mut input)?;
    Ok(())
}

fn interactive_get_tags() -> Result<Vec<String>, io::Error> {
    print!("Search (white-space separated): ");
    io::Write::flush(&mut io::stdout())?;
    let mut input = String::default();
    io::stdin().read_line(&mut input)?;
    Ok(input
        .split(" ")
        .map(str::trim)
        .map(str::to_string)
        .collect())
}

fn generate_tagger_pair(entry: &DirEntry) -> anyhow::Result<Option<(String, TaggerFile)>> {
    if !TAGGER_FILE_NAMES.contains(
        entry
            .file_name()
            .to_str()
            .context("{entry:?} filename not utf8")?,
    ) {
        return Ok(None);
    }

    let parent = entry
        .path()
        .parent()
        .map(|p| p.to_str())
        .flatten()
        .context("no parent found")?;

    Ok(Some((
        parent.to_string(),
        TaggerFile::new(std::fs::read_to_string(entry.path())?)?,
    )))
}

fn generate_taggers(dir: &Path) -> anyhow::Result<HashMap<String, TaggerFile>> {
    let mut taggers = HashMap::new();
    let mut dir_iter = WalkDir::new(dir).follow_links(false).into_iter();

    while let Some(Ok(entry)) = dir_iter.next() {
        if let Some((loc, f)) = generate_tagger_pair(&entry)? {
            taggers.insert(loc, f);
        }
    }

    Ok(taggers)
}

fn process_directory_tree(dir: &Path, tags: &Vec<String>) -> anyhow::Result<TaggedFiles> {
    let mut tag_hits = TaggedFiles::default();
    let taggers = generate_taggers(dir)?;

    let mut dir_iter = WalkDir::new(dir).follow_links(false).into_iter();

    while let Some(Ok(entry)) = dir_iter.next() {
        let parent = entry
            .path()
            .parent()
            .map(|p| p.to_str())
            .flatten()
            .context("no parent found")?;
        match taggers.get(parent) {
            Some(tagger_file) => {
                for tag in tags {
                    if let Some(ts) =
                        tagger_file.has_match(tag, entry.path())
                    {
                        for (t, hit) in ts {
                            tag_hits.add(t, hit.as_path())?;
                        }
                    }
                }
            }
            None => {}
        }
    }

    Ok(tag_hits)
}

#[derive(Default, Debug)]
struct TaggedFiles(HashMap<String, Vec<String>>);

impl TaggedFiles {
    fn add(&mut self, tag: &str, hit: &std::path::Path) -> Result<(), std::io::Error> {
        if let Some(hits) = self.0.get_mut(tag) {
            hits.push(hit.to_string_lossy().to_string());
        } else {
            self.0.insert(
                tag.to_string(),
                vec![hit.to_string_lossy().to_string()],
            );
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum TaggerLine {
    Tag(String, Vec<String>),
    DirTag(Vec<String>),
}

#[derive(Debug)]
struct TaggerFile(Vec<TaggerLine>);

impl TaggerFile {
    fn new(yaml: String) -> Result<Self, serde_yaml::Error> {
        Ok(Self(serde_yaml::from_str(&yaml)?))
    }

    fn has_match(&self, target_tag: &String, target_file: &Path) -> Option<Vec<(&String, PathBuf)>> {
        let target_tag = Regex::new(target_tag).unwrap();
        let target_filename = target_file.file_name()?.to_string_lossy();
        let mut matches = vec![];
        for line in &self.0 {
            match line {
                TaggerLine::Tag(f, tags) if target_file.is_file() => {
                    let filename_matcher = Regex::new(f).unwrap();
                    if !filename_matcher.is_match(&target_filename) {
                        return None;
                    }
                    for t in tags {
                        if target_tag.is_match(t) {
                            matches.push((t, target_file.to_path_buf()));
                        }
                    }
                }
                TaggerLine::DirTag(tags) => {
                    for t in tags {
                        if target_tag.is_match(t) {
                            matches.push((t, target_file.parent()?.to_path_buf()));
                        }
                    }
                }

                _ => {},
            }
        }

        if matches.is_empty() {
            None
        } else {
            Some(matches)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yaml_tagger() {
        let yaml = "- !Tag [foo.txt, [foo-tag]]";
        let tags: Vec<TaggerLine> = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(
            vec![TaggerLine::Tag(
                "foo.txt".to_string(),
                vec!["foo-tag".to_string()]
            )],
            tags
        );

        let yaml = "
        - !Tag 
            - bar.txt
            - [bar-tag]
        ";
        let tags: Vec<TaggerLine> = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(
            vec![TaggerLine::Tag(
                "bar.txt".to_string(),
                vec!["bar-tag".to_string()]
            )],
            tags
        );
    }
}
