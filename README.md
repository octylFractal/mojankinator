mojankinator
============
"Hey, Perry the Platypus, I've got a new invention to show you! It's called the Mojankinator! It takes all of Mojang's
janky code and produces a git repository with all of the code in a nice, clean, and organized manner! It's perfect for
diffing the code and seeing what's changed between versions! And it's all done with a single command! What do you
think?" - Dr. Doofenshmirtz

<details>
<summary>Open for more</summary>
"What? How does this help me take over the Tri-State Area? Well, you see, Perry the Platypus, if I can figure out
what Mojang is doing, I can figure out how to do it better! Then my game will be more popular than Minecraft, and
I'll be able to take over the Tri-State Area! It's foolproof!" - Dr. Doofenshmirtz
</details>

Usage
=====
This command expects to maintain all of its state in the directory where it is run. It uses a config file in this
directory named `mojankinator.toml`, which should contain the following fields:

```toml
# The minimum version of the game to store in the repository
min_version = "1.17.1"
# The maximum version of the game to store in the repository
max_version = "1.17.1"
# (Optional, default false) Should snapshots be included in the repository?
include_snapshots = true
```

The repository will be stored in `./repository`, each version commit will be tagged with the version number, and the
`HEAD` will be the latest version.

Decompilation work is stored in `./decompilationWorkArea`.

If you update the config file, `mojankinator` will update the repository with new versions or remove old versions. Do
not rely on a stable commit hash for any version, as the repository may be rewritten every time the config file is
updated.
