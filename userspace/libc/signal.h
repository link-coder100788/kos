#ifndef C_SIGNAL_H
#define C_SIGNAL_H

// Signals. For now only raise() sends them, to this program: the kernel
// doesn't deliver any yet (Ctrl+C and kill end a program outright).
// Without a handler, a signal ends the program with status 128 + its
// number, unless it's ignored (SIG_IGN, or SIGCHLD by default).

typedef int sig_atomic_t;
typedef void (*sighandler_t)(int);

#define SIG_DFL ((sighandler_t)0)
#define SIG_IGN ((sighandler_t)1)
#define SIG_ERR ((sighandler_t)-1)

// Linux's numbers.
#define SIGHUP 1
#define SIGINT 2
#define SIGQUIT 3
#define SIGILL 4
#define SIGTRAP 5
#define SIGABRT 6
#define SIGBUS 7
#define SIGFPE 8
#define SIGKILL 9
#define SIGUSR1 10
#define SIGSEGV 11
#define SIGUSR2 12
#define SIGPIPE 13
#define SIGALRM 14
#define SIGTERM 15
#define SIGCHLD 17
#define SIGCONT 18
#define SIGSTOP 19
#define SIGTSTP 20
#define SIGTTIN 21
#define SIGTTOU 22
#define SIGWINCH 28
#define NSIG 32

// Sets what signal `sig` does: a function to call, SIG_DFL or SIG_IGN.
// Returns what it did before, or SIG_ERR (SIGKILL and SIGSTOP can't change).
sighandler_t signal(int sig, sighandler_t handler);
// Sends `sig` to this program. 0, or -1 for a bad signal number.
int raise(int sig);

#endif //C_SIGNAL_H
