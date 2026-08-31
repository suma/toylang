/* NETWORK_IO.md N0 — the ground truth for the platform constants.
 *
 * `toylang_rt` is dependency-free, so every socket / poll constant in
 * `sys_epoll.rs` / `sys_kqueue.rs` is transcribed from these headers
 * by hand. A wrong transcription compiles cleanly and misbehaves at
 * run time; this program prints what the headers actually say so the
 * test can compare, which turns the worst failure shape into an
 * ordinary assertion.
 *
 * One `NAME value` per line. Names that exist on one platform only are
 * printed only there, and the test asks for the matching half. */

#include <stdio.h>
#include <stddef.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <netinet/in.h>

#ifdef __linux__
#include <sys/epoll.h>
#else
#include <sys/event.h>
#endif

int main(void) {
#ifdef __linux__
    printf("BACKEND %d\n", 1);
#else
    printf("BACKEND %d\n", 2);
#endif
    printf("AF_INET %d\n", AF_INET);
    printf("SOCK_STREAM %d\n", SOCK_STREAM);
    printf("SOCK_DGRAM %d\n", SOCK_DGRAM);
    printf("SOL_SOCKET %d\n", SOL_SOCKET);
    printf("SO_REUSEADDR %d\n", SO_REUSEADDR);
    printf("SO_RCVTIMEO %d\n", SO_RCVTIMEO);
    printf("SO_SNDTIMEO %d\n", SO_SNDTIMEO);
    printf("SO_ERROR %d\n", SO_ERROR);
    printf("O_NONBLOCK %d\n", O_NONBLOCK);
    printf("F_GETFL %d\n", F_GETFL);
    printf("F_SETFL %d\n", F_SETFL);
    printf("TIMEVAL_USEC_BYTES %zu\n",
           sizeof(((struct timeval *)0)->tv_usec));

    printf("EINTR %d\n", EINTR);
    printf("EAGAIN %d\n", EAGAIN);
    printf("EINVAL %d\n", EINVAL);
    printf("EMFILE %d\n", EMFILE);
    printf("EPIPE %d\n", EPIPE);
    printf("EADDRINUSE %d\n", EADDRINUSE);
    printf("EADDRNOTAVAIL %d\n", EADDRNOTAVAIL);
    printf("ENETUNREACH %d\n", ENETUNREACH);
    printf("ECONNABORTED %d\n", ECONNABORTED);
    printf("ECONNRESET %d\n", ECONNRESET);
    printf("ENOTCONN %d\n", ENOTCONN);
    printf("ETIMEDOUT %d\n", ETIMEDOUT);
    printf("ECONNREFUSED %d\n", ECONNREFUSED);
    printf("EHOSTUNREACH %d\n", EHOSTUNREACH);
    printf("EINPROGRESS %d\n", EINPROGRESS);

    /* `sockaddr_in` is 16 bytes on both, but the first two bytes mean
     * different things (BSD splits them into sin_len + sin_family).
     * The runtime builds it internally so toylang never sees one; the
     * size is checked because that is what a stack buffer for it has
     * to be. */
    printf("SOCKADDR_IN_BYTES %zu\n", sizeof(struct sockaddr_in));

#ifdef __linux__
    printf("EVENT_STRUCT_BYTES %zu\n", sizeof(struct epoll_event));
    printf("EPOLLIN %d\n", EPOLLIN);
    printf("EPOLLOUT %d\n", EPOLLOUT);
    printf("EPOLLERR %d\n", EPOLLERR);
    printf("EPOLLHUP %d\n", EPOLLHUP);
    printf("EPOLLRDHUP %d\n", EPOLLRDHUP);
    printf("EPOLLONESHOT %d\n", (int)EPOLLONESHOT);
    printf("EPOLLET %lld\n", (long long)(unsigned)EPOLLET);
    printf("EPOLL_CTL_ADD %d\n", EPOLL_CTL_ADD);
    printf("EPOLL_CTL_DEL %d\n", EPOLL_CTL_DEL);
    printf("EPOLL_CTL_MOD %d\n", EPOLL_CTL_MOD);
    printf("MSG_NOSIGNAL %d\n", MSG_NOSIGNAL);
#else
    printf("EVENT_STRUCT_BYTES %zu\n", sizeof(struct kevent));
    printf("EVFILT_READ %d\n", EVFILT_READ);
    printf("EVFILT_WRITE %d\n", EVFILT_WRITE);
    printf("EV_ADD %d\n", EV_ADD);
    printf("EV_DELETE %d\n", EV_DELETE);
    printf("EV_ONESHOT %d\n", EV_ONESHOT);
    printf("EV_CLEAR %d\n", EV_CLEAR);
    printf("EV_EOF %d\n", EV_EOF);
    printf("EV_ERROR %d\n", EV_ERROR);
    printf("SO_NOSIGPIPE %d\n", SO_NOSIGPIPE);
#endif
    return 0;
}
