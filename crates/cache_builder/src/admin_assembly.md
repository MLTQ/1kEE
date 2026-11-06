# admin_assembly.rs

Whole-planet administrative assembly, ordered from countries (level 2) to local
areas (11). Unlike focused merging, this path checkpoints every assembled relation
and publishes partial level snapshots while it works. It does not retain all
planet geometry in RAM. Only lightweight relation metadata is queued.

Completed regions and coordinates are reused on resume. Existing checkpoints are
republished at startup, the first region of each level publishes immediately, then
at 30-second intervals and each level's completion. The source is validated before
saving a relation. build-status.json exposes published/assembled/total counts and
completion so an unfinished cache is never presented as worldwide coverage.
An interruption after commit but before publication replays publication only.
