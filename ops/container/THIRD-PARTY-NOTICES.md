# Container security profiles

`seccomp.json` and `apparmor.profile` are adapted from Moby profiles at
commit `4b8214fc4971e06172938a1f1554ccff6647ee80` (https://github.com/moby/profiles).
The upstream Apache-2.0 license is reproduced in `MOBY-LICENSE`.

Changes: permit unprivileged nested user/mount namespaces for the runtime's
bubblewrap command isolation, rename the AppArmor profile and its peer labels,
and remove the upstream template substitutions. Docker capabilities remain
dropped. These are modified profiles, not the unmodified Docker defaults.
