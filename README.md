# tagger

Tag files/dirs via `.tagger.yaml` sidecar files. `tagger` performs recursive search,
looking for `.tagger.yaml` or `tagger.yaml` files from each of the provided file trees.

## Features

- Sidecar `.tagger.yaml` tags.
- File & directory tagging.
- Regex tag search.
- Interactive mode.

# tagger YAML syntax

```
- !Tag [file-regex, [tags]] # sibling files, but not directories
- !DirTag [tags] # tags `.`, ie the current directory
```

# Example

```
- !DirTag [project]
- !Tag ["\\.txt$", [text-files]]
- !Tag ["\\.png$", [group-trip, photos]]
```

# Config files

You can put a settings file at `~/.config/tagger/settings.yaml` to pre-configure directories:

```
dirs: [/Users/username/one, /Users/username/two/*]
```

Globbing should work as it does in the shell.

