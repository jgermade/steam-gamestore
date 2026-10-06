/*
 * depth16.exe <game.exe> [arguments...]
 *
 * Switch the display to 16-bit colour for as long as a game runs, then put it
 * back. What GOG's launchers for late-90s games do before starting them: such a
 * game checks the desktop's colour depth on start and refuses anything but 16
 * bits ("16 bit screen mode required"). Under Wine the switch is emulated, which
 * is all the check needs.
 *
 * No C runtime: it is built with clang and lld-link from build.sh, against import
 * libraries made from the two .def files, so no MinGW is needed.
 */

typedef unsigned short WORD;
typedef unsigned long DWORD;
typedef int BOOL;
typedef void *HANDLE;

typedef struct {
    char dmDeviceName[32];
    WORD dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
    DWORD dmFields;
    char dmUnion[16];
    short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
    char dmFormName[32];
    WORD dmLogPixels;
    DWORD dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
    DWORD dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
    DWORD dmPanningWidth, dmPanningHeight;
} DEVMODEA;

typedef struct {
    DWORD cb;
    char *lpReserved, *lpDesktop, *lpTitle;
    DWORD dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
    WORD wShowWindow, cbReserved2;
    void *lpReserved2;
    HANDLE hStdInput, hStdOutput, hStdError;
} STARTUPINFOA;

typedef struct {
    HANDLE hProcess, hThread;
    DWORD dwProcessId, dwThreadId;
} PROCESS_INFORMATION;

__declspec(dllimport) BOOL __stdcall EnumDisplaySettingsA(const char *, DWORD, DEVMODEA *);
__declspec(dllimport) long __stdcall ChangeDisplaySettingsA(DEVMODEA *, DWORD);
__declspec(dllimport) char *__stdcall GetCommandLineA(void);
__declspec(dllimport) BOOL __stdcall CreateProcessA(const char *, char *, void *, void *, BOOL,
                                                    DWORD, void *, const char *, STARTUPINFOA *,
                                                    PROCESS_INFORMATION *);
__declspec(dllimport) DWORD __stdcall WaitForSingleObject(HANDLE, DWORD);
__declspec(dllimport) BOOL __stdcall GetExitCodeProcess(HANDLE, DWORD *);
__declspec(dllimport) void __stdcall ExitProcess(unsigned int);

#define ENUM_CURRENT_SETTINGS ((DWORD)-1)
#define DM_BITSPERPEL 0x00040000
#define DM_PELSWIDTH 0x00080000
#define DM_PELSHEIGHT 0x00100000
#define CDS_FULLSCREEN 0x00000004
#define INFINITE 0xFFFFFFFF

/* Skip the first word of a command line, quoted or not. */
static char *after_program(char *line) {
    if (*line == '"') {
        for (line++; *line && *line != '"'; line++) {}
        if (*line) line++;
    } else {
        while (*line && *line != ' ' && *line != '\t') line++;
    }
    while (*line == ' ' || *line == '\t') line++;
    return line;
}

void start(void) {
    char *game = after_program(GetCommandLineA());
    if (!*game) ExitProcess(2);

    DEVMODEA mode;
    for (unsigned i = 0; i < sizeof mode; i++) ((char *)&mode)[i] = 0;
    mode.dmSize = sizeof mode;
    BOOL known = EnumDisplaySettingsA(0, ENUM_CURRENT_SETTINGS, &mode);
    if (known) {
        mode.dmBitsPerPel = 16;
        /* The size has to be named too: with the depth alone Wine accepts the
         * change, yet only this process sees it, not the game it starts. */
        mode.dmFields = DM_BITSPERPEL | DM_PELSWIDTH | DM_PELSHEIGHT;
        /* CDS_FULLSCREEN: temporary, undone when this process ends. */
        ChangeDisplaySettingsA(&mode, CDS_FULLSCREEN);
    }

    STARTUPINFOA startup;
    PROCESS_INFORMATION process;
    for (unsigned i = 0; i < sizeof startup; i++) ((char *)&startup)[i] = 0;
    startup.cb = sizeof startup;
    DWORD code = 1;
    if (CreateProcessA(0, game, 0, 0, 0, 0, 0, 0, &startup, &process)) {
        WaitForSingleObject(process.hProcess, INFINITE);
        GetExitCodeProcess(process.hProcess, &code);
    }
    if (known) ChangeDisplaySettingsA(0, 0);
    ExitProcess(code);
}
