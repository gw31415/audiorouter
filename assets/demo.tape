Output assets/demo.gif

Set Shell "bash"
Set FontSize 24
Set FontFamily "Menlo"
Set Width 1400
Set Height 900
Set Theme "Catppuccin Mocha"
Set Padding 30
Set Framerate 30

# Launch audiorouter without showing the command.
Hide
Type "./target/debug/audiorouter -c assets/config.toml" Enter
Sleep 3s
Show

# Record only the running TUI.
Sleep 8s

# Quit without showing the shell prompt.
Hide
Ctrl+C
Sleep 1s
