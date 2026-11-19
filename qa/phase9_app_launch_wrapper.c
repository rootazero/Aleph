#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2 || !argv[1][0]) {
        dprintf(STDERR_FILENO, "phase9 launcher requires a target executable\n");
        return 64;
    }

    const char *stdout_path = getenv("ALEPH_PHASE9_STDOUT_PATH");
    const char *stderr_path = getenv("ALEPH_PHASE9_STDERR_PATH");
    if (!stdout_path || !stderr_path) {
        dprintf(STDERR_FILENO, "phase9 launcher requires capture paths\n");
        return 64;
    }

    int out = open(stdout_path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    int err = open(stderr_path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (out < 0 || err < 0 || dup2(out, STDOUT_FILENO) < 0 || dup2(err, STDERR_FILENO) < 0) {
        dprintf(STDERR_FILENO, "phase9 launcher could not open capture files\n");
        return 73;
    }
    close(out);
    close(err);

    execv(argv[1], &argv[1]);
    dprintf(STDERR_FILENO, "phase9 target exec failed\n");
    return 126;
}
