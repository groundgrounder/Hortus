# Working in a Hortus knowledge base

Hortus shows a folder of Markdown notes. Treat the folder as the whole
knowledge base, in the user's preferred language and voice.

## Notes

- One note covers one topic. The file name is the title; do not repeat the title
  as a heading unless the note already does.
- Before creating a note, search the folder for an existing note on the topic.
  Extend that note instead of starting a near-duplicate.
- Reference another note with `[[Note name]]` and rely on backlinks. Do not copy
  the referenced text into the note.
- Use frontmatter sparingly: `tags`, `aliases`, and optional `source`. Leave
  other frontmatter fields exactly as they are.
- When editing, change only the paragraphs related to the request. Keep the
  existing heading order, list markers, spacing, and line-ending style so the
  diff stays small and easy to review. Do not reformat or re-wrap the note.
- Keep images and attachments where they are; link them with normal Markdown.
- Write plain Markdown. Do not add HTML beyond what is described below.

## Diagrams

- Prefer Mermaid `mindmap` or a single-root `flowchart` that forms a tree; those
  layouts read best in Hortus.
- Put the main term first: `Main term  short explanation`. Use `<br/>` to add a
  secondary line inside a node label.
- Avoid `subgraph` and diagram types that Hortus does not render yet. Keep the
  diagram source small enough to read in one screen.
- Keep node labels short and parallel in style; do not encode entire paragraphs
  in nodes.

## Files

- Never write editor or application state, caches, or indexes into the
  knowledge base.
- Do not create an `AGENTS.md` or other instruction file in the knowledge base.
