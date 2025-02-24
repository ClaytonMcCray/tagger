use anyhow::Context;
use clap::Parser;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use walkdir::{DirEntry, WalkDir};

static TAGGER_FILE_NAMES: Lazy<HashSet<&'static str>> =
    Lazy::new(|| HashSet::from([".tagger.yaml", "tagger.yaml"]));

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// The directories to operate on.
    #[arg(required = true)]
    dirs: Vec<std::path::PathBuf>,

    /// Regular expressions representing the tags to match on.
    #[arg(required = true, last = true)]
    tags: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let results = args
        .dirs
        .iter()
        .map(|path| process_directory_tree(path, &args.tags))
        .collect::<anyhow::Result<Vec<TaggedFiles>>>()?;

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

    Ok(())
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
    let mut dir_iter = WalkDir::new(dir).into_iter();

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

    let mut dir_iter = WalkDir::new(dir).into_iter();

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
                        tagger_file.has_match(tag, &entry.file_name().to_string_lossy())
                    {
                        for t in ts {
                            tag_hits.add(t, entry.path())?;
                        }
                    }
                }
            }
            None => {
                for tag in tags {
                    let Some(tagger) = TaggerFile::with_dir_tag(&entry) else {
                        // not a dir
                        continue;
                    };

                    if let Some(ts) = tagger.has_match(tag, &entry.file_name().to_string_lossy()) {
                        for t in ts {
                            tag_hits.add(t, entry.path())?;
                        }
                    }
                }
            }
        }
    }

    Ok(tag_hits)
}

#[derive(Default, Debug)]
struct TaggedFiles(HashMap<String, Vec<String>>);

impl TaggedFiles {
    fn add(&mut self, tag: &str, hit: &std::path::Path) -> Result<(), std::io::Error> {
        if let Some(hits) = self.0.get_mut(tag) {
            hits.push(hit.canonicalize()?.to_string_lossy().to_string());
        } else {
            self.0.insert(
                tag.to_string(),
                vec![hit.canonicalize()?.to_string_lossy().to_string()],
            );
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum TaggerLine {
    Tag(String, Vec<String>),
}

#[derive(Debug)]
struct TaggerFile(Vec<TaggerLine>);

impl TaggerFile {
    fn new(yaml: String) -> Result<Self, serde_yaml::Error> {
        Ok(Self(serde_yaml::from_str(&yaml)?))
    }

    fn with_dir_tag(target: &DirEntry) -> Option<Self> {
        if target.path().is_dir() {
            return None;
        }

        Some(Self(vec![TaggerLine::Tag(
            target.file_name().to_string_lossy().to_string(),
            vec![target
                .path()
                .canonicalize()
                .ok()?
                .parent()?
                .file_name()?
                .to_string_lossy()
                .to_string()],
        )]))
    }

    fn has_match(&self, target_tag: &String, target_filename: &str) -> Option<Vec<&String>> {
        let target_tag = Regex::new(target_tag).unwrap();
        let mut matches = vec![];
        for line in &self.0 {
            match line {
                TaggerLine::Tag(f, tags) => {
                    if f != target_filename {
                        return None;
                    }
                    for t in tags {
                        if target_tag.is_match(t) {
                            matches.push(t);
                        }
                    }
                }
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
