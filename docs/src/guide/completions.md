# Shell Completions

`audiorouter` can generate completion scripts for any shell supported by
[`clap_complete`]: bash, elvish, fish, powershell, and zsh.

```sh
# Write to stdout and source immediately (fish example)
audiorouter completions fish | source

# Write to a file
audiorouter completions bash --output ~/.bash_completion.d/audiorouter
```

When no shell is given, the current shell is detected from `$SHELL`.
