# Implementation Plan Format

This document describes the format of the implemention plan and management of the plan.

The `plan.md` file should be used as the top level plan. It should contain an `Overview` section describing the overall plan as well as a table of content with an entry for each phase of development.

A plan consists of a set of Phases each containing a set of Milestones each containing a set of tasks. Tasks, milestones, and phase entries may use checklist markers with the following meanings:

- `[ ]` not started
- `[x]` complete
- `[/]` partially complete

There shall be one file per phase, named `<num>-phase-<name>.md` containing all milestones and tasks for that phase. `<num>` shall be an integer ordering of phases, and the top-level plan order shall reflect the intended implementation order.

Do not use emojis.
