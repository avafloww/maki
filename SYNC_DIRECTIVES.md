this is lunamaki, a fork of maki

remotes:
- fork (lunamaki)
- origin (upstream maki)

the job here is to synchronize lunamaki with upstream maki in an additive manner, never removing features from lunamaki itself.

to do this, classify the differences between origin/main and current branch. decide on what to do for each conflict, then
work to do a merge from origin/main to current branch in a way that doesn't cause any regressions

there are things on the merges that we should always defer to fork (like branding and version)
after investigating create a subagent, put your investigations on it, make the subagent run through the changes themselves, and then report back to you when the code edits are done.

then report back to me on which commands you'd want to run. i got a different build machine that should be faster than the VM you're on
