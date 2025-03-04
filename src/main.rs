use anyhow::Context;
use clap::Parser;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

static TAGGER_FILE_NAMES: Lazy<HashSet<&'static str>> =
    Lazy::new(|| HashSet::from([".tagger.yaml", "tagger.yaml"]));

#[derive(Parser, Serialize, Deserialize, PartialEq, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The directories to operate on. May be left out only if a config at
    /// ~/.config/tagger/settings.yaml specifies the dirs.
    #[arg(long, short)]
    dirs: Option<Vec<std::path::PathBuf>>,

    #[arg(long, action=clap::ArgAction::SetTrue)]
    #[serde(default)]
    or: bool,

    /// Regular expressions representing the tags to match on.
    /// Leave out for interactive mode.
    #[arg()]
    tags: Option<Vec<String>>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let args = if args.dirs.is_none() {
        let mut new_args: Args = serde_yaml::from_str(&std::fs::read_to_string(
            home::home_dir()
                .expect("home dir")
                .join(".config/tagger/settings.yaml"),
        )?)?;
        new_args.dirs = new_args.dirs.map(|dirs| {
            dirs.into_iter()
                .flat_map(|d| match glob::glob(&d.to_string_lossy()) {
                    Ok(paths) => paths.filter_map(Result::ok).collect::<Vec<_>>(),
                    Err(e) => {
                        eprintln!("Error expanding glob pattern '{}': {}", d.display(), e);
                        Vec::new()
                    }
                })
                .collect()
        });
        new_args.tags = args.tags;
        new_args.or = if args.or { args.or } else { new_args.or };
        new_args
    } else {
        args
    };

    let (interactive, raw_tags) = match args.tags {
        Some(tags) => (false, tags),
        None => (true, interactive_get_tags()?),
    };

    let tags = raw_tags
        .iter()
        .map(|t| Regex::new(t))
        .collect::<Result<Vec<_>, _>>()?;

    let results = args
        .dirs
        .unwrap()
        .iter()
        .filter_map(|path| match path.canonicalize() {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!("error canonicalizing: {e:?}");
                None
            }
        })
        .filter_map(|path| match process_directory_tree(&path, &tags) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("error processing tree {path:?}: {e:?}");
                None
            }
        })
        .collect::<Vec<TaggedFiles>>();

    let mut deduplicated = BTreeMap::new();
    for tagged_file in results.into_iter() {
        for (k, v) in tagged_file.0.into_iter() {
            deduplicated
                .entry(k)
                .and_modify(|existing: &mut BTreeSet<String>| existing.extend(v.clone()))
                .or_insert(BTreeSet::from_iter(v.into_iter()));
        }
    }

    if args.or {
        println!("{}", serde_yaml::to_string(&deduplicated)?);
    } else {
        println!(
            "{}",
            serde_yaml::to_string(&BTreeMap::from_iter([(
                raw_tags.join(", "),
                get_intersection_of_tag_hits(deduplicated)
            )]))?
        );
    }

    if interactive {
        wait_for_input_to_quit()?;
    }

    Ok(())
}

fn wait_for_input_to_quit() -> Result<(), io::Error> {
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

fn get_intersection_of_tag_hits(map: BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    map.into_values()
        .reduce(|acc, set| acc.intersection(&set).cloned().collect())
        .unwrap_or_default()
}

fn generate_tagger_pair(entry: &DirEntry) -> anyhow::Result<Option<(PathBuf, TaggerFile)>> {
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
        .context("no parent found")?
        .canonicalize()?;

    Ok(Some((
        parent,
        TaggerFile::new(std::fs::read_to_string(entry.path())?)?,
    )))
}

fn generate_taggers(dir: &Path) -> anyhow::Result<HashMap<PathBuf, TaggerFile>> {
    let mut taggers = HashMap::new();
    let mut dir_iter = WalkDir::new(dir).follow_links(false).into_iter();

    while let Some(Ok(entry)) = dir_iter.next() {
        if let Some((loc, f)) = generate_tagger_pair(&entry)? {
            taggers.insert(loc, f);
        }
    }

    Ok(taggers)
}

fn process_directory_tree(dir: &Path, tags: &Vec<Regex>) -> anyhow::Result<TaggedFiles> {
    let mut tag_hits = TaggedFiles::default();
    let taggers = generate_taggers(dir)?;

    for (tagger_root, tagger_file) in taggers {
        for entry in tagger_root.read_dir()? {
            let entry = entry?;
            if TAGGER_FILE_NAMES.contains(
                entry
                    .file_name()
                    .to_str()
                    .context("{entry:?} filename not utf8")?,
            ) {
                continue;
            }

            for tag in tags {
                if let Some(ts) = tagger_file.has_match(tag, &entry.path()) {
                    for (t, hit) in ts {
                        tag_hits.add(t, hit.as_path())?;
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
            hits.push(hit.to_string_lossy().to_string());
        } else {
            self.0
                .insert(tag.to_string(), vec![hit.to_string_lossy().to_string()]);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum TaggerLineRaw {
    Tag(String, Vec<String>),
    DirTag(Vec<String>),
}

#[derive(Debug)]
enum TaggerLine {
    Tag(Regex, Vec<String>),
    DirTag(Vec<String>),
}

impl TryFrom<TaggerLineRaw> for TaggerLine {
    type Error = regex::Error;
    fn try_from(input: TaggerLineRaw) -> Result<TaggerLine, Self::Error> {
        match input {
            TaggerLineRaw::Tag(f, tags) => Ok(TaggerLine::Tag(Regex::new(&f)?, tags)),
            TaggerLineRaw::DirTag(tags) => Ok(TaggerLine::DirTag(tags)),
        }
    }
}

#[derive(Debug)]
struct TaggerFile(Vec<TaggerLine>);

impl TaggerFile {
    fn new(yaml: String) -> Result<Self, serde_yaml::Error> {
        let lines: Vec<TaggerLineRaw> = serde_yaml::from_str(&yaml)?;
        Ok(Self(
            lines
                .into_iter()
                .filter_map(|line| match TaggerLine::try_from(line) {
                    Ok(line) => Some(line),
                    Err(e) => {
                        eprintln!("error processing tagger file: {e:?}");
                        None
                    }
                })
                .collect(),
        ))
    }

    fn has_match(&self, target_tag: &Regex, target_file: &Path) -> Option<Vec<(&String, PathBuf)>> {
        let target_filename = target_file.file_name()?.to_string_lossy();
        let mut matches = vec![];
        for line in &self.0 {
            match line {
                TaggerLine::Tag(f, tags) if target_file.is_file() => {
                    if !f.is_match(&target_filename) {
                        continue;
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

                _ => {}
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
