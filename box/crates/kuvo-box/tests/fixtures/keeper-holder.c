#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void path(char *out, const char *home, const char *name) {
    snprintf(out, MAX_PATH, "%s\\run\\%s", home, name);
}
static int read_number(const char *file) {
    FILE *in = fopen(file, "r");
    int result = 0;
    if (in) {
        fscanf(in, "%d", &result);
        fclose(in);
    }
    return result;
}
static void write_number(const char *file, int n) {
    FILE *out = fopen(file, "w");
    if (!out) exit(20);
    fprintf(out, "%d\n", n);
    fclose(out);
}
int main(int argc, char **argv) {
    char home[MAX_PATH], holder[MAX_PATH], turn[MAX_PATH], answer[MAX_PATH];
    if (!GetEnvironmentVariableA("KUVO_HOME", home, sizeof(home))) return 10;
    path(holder, home, "test-holder.pid");
    path(turn, home, "test-turn");
    path(answer, home, "test-answer");

    if (argc == 2 && strcmp(argv[1], "holder") == 0) {
        int done = 0;
        for (;;) {
            int requested = read_number(turn);
            if (requested > done) {
                write_number(answer, requested);
                done = requested;
            }
            Sleep(20);
        }
    }

    DWORD pid = read_number(holder);
    HANDLE old = pid ? OpenProcess(SYNCHRONIZE, FALSE, pid) : NULL;
    if (old && WaitForSingleObject(old, 0) == WAIT_TIMEOUT) {
        char adopted[MAX_PATH];
        path(adopted, home, "test-adopted.pid");
        write_number(adopted, pid);
    } else {
        char program[MAX_PATH], command[MAX_PATH + 16];
        GetModuleFileNameA(NULL, program, sizeof(program));
        snprintf(command, sizeof(command), "\"%s\" holder", program);
        STARTUPINFOA startup = {.cb = sizeof(startup)};
        PROCESS_INFORMATION child;
        if (!CreateProcessA(program, command, NULL, NULL, FALSE, CREATE_NO_WINDOW, NULL, NULL, &startup, &child)) return GetLastError();
        write_number(holder, child.dwProcessId);
        CloseHandle(child.hThread);
        CloseHandle(child.hProcess);
    }
    if (old) CloseHandle(old);
    char resident[MAX_PATH];
    path(resident, home, "test-resident.pid");
    write_number(resident, GetCurrentProcessId());
    for (;;) Sleep(1000);
}
