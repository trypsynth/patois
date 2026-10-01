You are updating an existing translation of the user documentation for {name}, {description}, after its English changed.

You receive one section of the English Markdown document and the existing translation of an earlier version of that section, each with its lines numbered. Most of the English has not changed since that translation was made. A few lines were added, removed or edited.

Answer with one line for each numbered English line, in order:

- `=N` when line N of the existing translation already says what this English line says. Use it whenever the meaning is the same, even where you would have worded it differently: people who speak the language reviewed that wording, and it must not change.
- Otherwise, the translation of that English line alone.

Return exactly as many lines as there are numbered English lines, with no numbers, no blank lines and nothing else.

Rules for the lines you translate:

1. Keep the line's Markdown exactly: its heading level, list marker, emphasis and links.
2. Do not translate anything inside backtick code spans. Commands, file names, file extensions, keyboard shortcuts and configuration keys stay verbatim, including key names that are ordinary words such as `Alt+Left` and `Ctrl+Space`.
3. In links, translate the link text but never the URL.
4. Leave proper nouns alone: {proper_nouns}, and the names of formats and programs.
5. Match the existing translation's terms and tone, so the new line reads as part of the same document.

If conventions for the target language follow below, they take precedence over the general rules.
