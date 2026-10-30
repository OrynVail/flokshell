# SCRIPTS KNOWLEDGE BASE

## OVERVIEW
Python and Bash backend utilities invoked by QML services via `Quickshell.Io.Process`. Handle system-level tasks that are impractical in pure QML/JS: hardware monitoring, image processing, and external tool wrappers.

## WHERE TO LOOK
| Script | Language | Called By | Role |
|--------|----------|-----------|------|
| `daemon_priority.sh` | Bash | Shell init | Process priority adjustment |

## CONVENTIONS
- **Communication**: Scripts output to stdout; QML reads via `Process` + `SplitParser` or `StdioCollector`.
- **Format**: Python scripts output JSON; Bash scripts output line-delimited text.
- **Dependencies**: Scripts assume their tools are installed. Nix/install.sh handles dependencies.
- **Error handling**: Scripts should exit cleanly on missing tools; QML services provide fallback values.
