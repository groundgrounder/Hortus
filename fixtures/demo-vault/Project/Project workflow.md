# Project workflow

A small project has a question, an experiment, and a decision. Keep the result in [[Weekly review]].

```mermaid
flowchart TD
  question[Choose a question] --> plan[Plan a small experiment]
  plan --> trial[Run the experiment]
  trial --> check{Useful evidence?}
  check -->|Yes| explain[Write what changed]
  check -->|Not yet| revise[Adjust one variable]
  revise --> next[Plan another trial]
  explain --> share[Share a short note]
```

- [x] Define what a useful result would look like.
- [ ] Keep the experiment small enough for one session.
- [ ] Link the result back to the question.

Content: CC0 1.0 Universal.
