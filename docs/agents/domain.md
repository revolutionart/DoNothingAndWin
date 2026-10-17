# Domain documentation

This launcher uses a single-context layout: `GLOSSARY.md` and `docs/adr/` at the repository root.

Before exploring the codebase, read the glossary if it exists and the ADRs relevant to the area being changed. If these files are absent, proceed silently. Create glossary entries and ADRs when domain-modeling work resolves actual terminology or decisions; empty starter files are unnecessary.

Use glossary terms consistently in specs, tickets, hypotheses, tests, and code. If a needed term is missing, first check whether an existing term already covers it; record a genuine gap through the domain-modeling workflow.

Store architectural decisions as numbered files under `docs/adr/`, such as `0001-descriptive-decision-title.md`. Surface any conflict with an existing ADR explicitly, with its path and the reason for reconsidering it.
