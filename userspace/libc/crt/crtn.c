// Empty: tcc links crti.o and crtn.o around every program, as on Linux,
// where they hold the start and end of .init and .fini. KOS has neither.
