# Evidence and Work Modes

## Choose the mode before acting

| Mode | Authorized work | Required output |
| --- | --- | --- |
| Add | Implement a new provider end to end | Evidence map, cross-layer implementation, tests, real validation, gates, and remaining source limits |
| Extend | Add or correct capabilities in an existing provider | Baseline evidence, scoped behavior diff, regression matrix, and unchanged capability checks |
| Audit | Read current source, types, tests, real artifacts, and runtime state without product changes | Findings ranked by correctness impact, exact evidence, coverage gaps, and recommended fixes |
| Repair | Diagnose and implement user-authorized fixes | Reproduced failure, repaired contract, regression tests, real/runtime validation, and scoped diff |

Audit mode is read-only unless the user explicitly asks to repair findings. Documentation or skill edits are also writes and require that authorization. Add, extend, and repair do not authorize commits, pushes, releases, target-CLI execution, or mutation of the user's primary SessionView index unless separately requested or inherently required and safely isolated.

## Evidence-state vocabulary

Use these states consistently for every capability, especially subagents, images, usage, resume, and schema variants:

| State | Meaning |
| --- | --- |
| Confirmed persisted | A real local artifact contains the typed data and the parser/runtime path was verified |
| Writer-supported but unobserved | Current writer/source/types prove persistence behavior, but the inspected local corpus has no representative artifact |
| Confirmed absent | Writer/source and controlled artifacts prove the capability is not persisted or the required field is deliberately omitted |
| Remote-only | The capability exists during service execution but is not written to the local source graph SessionView can read |
| Unknown | Available evidence cannot establish the behavior safely |

Never collapse “not found in these sessions” into “unsupported.” Never promote a type declaration or documentation claim to confirmed persistence without following the writer path or a real artifact. State version boundaries when evidence differs between installed and current source.

## Acceptance matrix

Maintain a working matrix with one row per capability and source input:

| Concern | Authority | Evidence state | Synthetic regression | Real read-only proof | Isolated runtime/UI proof | Remaining limit |
| --- | --- | --- | --- | --- | --- | --- |
| discovery/freshness | writer paths and every mutable file/DB | ... | mutation per source | current tree/schema | reindex round trip | ... |
| messages/tools/images | wire types and real blocks | ... | valid, malformed, unknown | representative sessions | rendered timeline/tool/image | ... |
| subagents | typed ownership/call ids | ... | root/child/nested/orphan | persisted relationship | Open subagent/parent navigation | ... |
| usage/cost | accounting writer and scope | ... | totals/model/time/scope | real usage rows | indexed analytics/totals | ... |
| resume | installed help and descriptor | ... | command shape/security | current CLI syntax | UI command only unless execution authorized | ... |

The final handoff summarizes this matrix without reproducing private evidence. A green compile, one parsed session, an installed executable, or a successful launch proves only its own column.
