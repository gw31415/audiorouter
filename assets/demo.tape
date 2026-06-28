Output demo.gif

Set Shell "bash"
Set FontSize 24
Set FontFamily "Menlo"
Set Width 1400
Set Height 900
Set Theme "Catppuccin Mocha"
Set Padding 30
Set Framerate 30

# Launch audiorouter without showing the command
Hide
Type "audiorouter -c config.toml" Enter
Sleep 3s
Show

# Record the running TUI
Sleep 6s

# Quit (hidden so the prompt doesn't appear in the GIF)
Hide
Ctrl+C
Sleep 1s
