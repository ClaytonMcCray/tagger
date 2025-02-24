use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use walkdir::WalkDir;

static TAGGER_FILE_NAMES: Lazy<HashSet<&'static str>> =
    Lazy::new(|| HashSet::from([".tagger.yaml", "tagger.yaml"]));

fn main() -> anyhow::Result<()> {
    let results =
        process_directory_tree("./example", vec!["src".to_string(), "test-tag".to_string()]);
    println!("{results:?}");
    Ok(())
}

fn process_directory_tree(dir: &str, tags: Vec<String>) -> TaggedFiles {
    let mut taggers = HashMap::new();
    let mut cache_misses = HashSet::new();
    let mut tag_hits = TaggedFiles::default();

    for entry in WalkDir::new(dir) {
        let Ok(entry) = entry else {
            eprintln!("error reading a file, dont know which one");
            continue;
        };

        let Some(current_name) = entry.file_name().to_str() else {
            eprintln!("{entry:?} does not have a utf8 filename");
            continue;
        };

        let Some(parent) = entry.path().parent().map(|p| p.to_str()).flatten() else {
            eprintln!("{entry:?} does not have a parent");
            continue;
        };

        if TAGGER_FILE_NAMES.contains(current_name) {
            let Ok(tagger_yaml) = std::fs::read_to_string(entry.path()) else {
                eprintln!("{entry:?} could not be read");
                continue;
            };

            match TaggerFile::new(tagger_yaml) {
                Ok(file) => {
                    taggers.insert(parent.to_string(), file);
                }
                Err(e) => eprintln!("failed to read tagger file {entry:?}: {e:?}"),
            }
        } else {
            match taggers.get(parent) {
                Some(tagger_file) => {
                    for tag in &tags {
                        if tagger_file.has_match(tag, &entry.file_name().to_string_lossy()) {
                            let Ok(()) = tag_hits.add(tag, entry.path()) else {
                                eprintln!("failed to add tag hit {entry:?} for {tag}");
                                continue;
                            };
                        }
                    }
                }
                None => {
                    let Ok(miss) = entry.clone().into_path().canonicalize() else {
                        eprintln!(
                            "skipping cache miss {entry:?} because it failed to canonicalize"
                        );
                        continue;
                    };
                    cache_misses.insert(miss);
                }
            }
        }
    }

    for target in cache_misses {
        match taggers.get(&target.parent().unwrap().to_string_lossy().to_string()) {
            Some(tagger_file) => {
                for tag in &tags {
                    if tagger_file.has_match(tag, &target.file_name().unwrap().to_string_lossy()) {
                        let Ok(()) = tag_hits.add(tag, target.as_path()) else {
                            eprintln!("failed to add tag hit {target:?} for {tag}");
                            continue;
                        };
                    }
                }
            }
            None => {
                let Some(parent_file_name) = target.parent().map(|p| p.file_name()).flatten()
                else {
                    eprintln!("failed to extract file name from parent of {target:?}");
                    continue;
                };

                for tag in &tags {
                    if TaggerFile::from_dir(
                        target.to_string_lossy().to_string(),
                        target.parent().unwrap(),
                    )
                    .unwrap()
                    .has_match(tag, &target.to_string_lossy())
                    {
                        let Ok(()) =
                            tag_hits.add(&parent_file_name.to_string_lossy(), target.as_path())
                        else {
                            eprintln!("failed to add {target:?} with tag of parent directory");
                            continue;
                        };
                    }
                }
            }
        }
    }

    tag_hits
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

    fn from_dir(target: String, dir: &std::path::Path) -> Option<Self> {
        if !dir.is_dir() {
            return None;
        }

        let s = Some(Self(vec![TaggerLine::Tag(
            target,
            vec![dir.file_name().unwrap().to_string_lossy().to_string()],
        )]));

        println!("{s:?}");
        s
    }

    fn has_match(&self, target_tag: &String, target_filename: &str) -> bool {
        for line in &self.0 {
            match line {
                TaggerLine::Tag(f, tags) => {
                    if f == target_filename && tags.contains(target_tag) {
                        return true;
                    }
                }
            }
        }

        return false;
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
