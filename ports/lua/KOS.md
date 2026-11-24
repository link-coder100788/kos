# Lua on KOS

Lua 5.4.7 from <https://www.lua.org/ftp/lua-5.4.7.tar.gz> (sha256
`9fbf5e28ef86c69858f6d3d34eccc32e911c1a28b4120ff3e84aaa70cfbf1e30`), `src/`
and `README` as they come, minus `src/Makefile`. Lua is MIT-licensed: the
licence is at the end of `src/lua.h`.

Nothing in it is changed for KOS: `make user` builds `src/*.c` (all but
`luac.c`) against `userspace/libc` with Lua's default, plain-C
configuration, into `/bin/lua`. It is the yardstick for KOS's POSIX work:
what it needs, the C library has to have.
