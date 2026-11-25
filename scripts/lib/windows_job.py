"""Run Windows children in a job before their first instruction executes."""

import ctypes
import subprocess
from ctypes import wintypes

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
INVALID_HANDLE = ctypes.c_void_p(-1).value


def bind(name, result, *args):
    function = getattr(kernel, name)
    function.restype = result
    function.argtypes = args
    return function


HANDLE, DWORD, BOOL = wintypes.HANDLE, wintypes.DWORD, wintypes.BOOL
POINTER = ctypes.c_void_p
U64 = ctypes.c_uint64


class BasicLimit(ctypes.Structure):
    _fields_ = [
        ("process_time", ctypes.c_int64), ("job_time", ctypes.c_int64),
        ("flags", DWORD), ("min_working_set", ctypes.c_size_t),
        ("max_working_set", ctypes.c_size_t), ("process_limit", DWORD),
        ("affinity", ctypes.c_size_t), ("priority", DWORD), ("scheduling", DWORD),
    ]


class Limits(ctypes.Structure):
    _fields_ = [
        ("basic", BasicLimit), ("counters", U64 * 6),
        ("process_memory", ctypes.c_size_t), ("job_memory", ctypes.c_size_t),
        ("peak_process_memory", ctypes.c_size_t), ("peak_job_memory", ctypes.c_size_t),
    ]


class Accounting(ctypes.Structure):
    _fields_ = [
        ("times", U64 * 4), ("faults", DWORD), ("total", DWORD),
        ("active", DWORD), ("terminated", DWORD),
    ]


class ThreadEntry(ctypes.Structure):
    _fields_ = [
        ("size", DWORD), ("usage", DWORD), ("thread", DWORD), ("process", DWORD),
        ("priority", wintypes.LONG), ("delta", wintypes.LONG), ("flags", DWORD),
    ]


create_job = bind("CreateJobObjectW", HANDLE, POINTER, wintypes.LPCWSTR)
set_information = bind("SetInformationJobObject", BOOL, HANDLE, ctypes.c_int, POINTER, DWORD)
query_information = bind("QueryInformationJobObject", BOOL, HANDLE, ctypes.c_int, POINTER, DWORD, POINTER)
assign = bind("AssignProcessToJobObject", BOOL, HANDLE, HANDLE)
terminate = bind("TerminateJobObject", BOOL, HANDLE, wintypes.UINT)
close = bind("CloseHandle", BOOL, HANDLE)
snapshot_threads = bind("CreateToolhelp32Snapshot", HANDLE, DWORD, DWORD)
first_thread = bind("Thread32First", BOOL, HANDLE, POINTER)
next_thread = bind("Thread32Next", BOOL, HANDLE, POINTER)
open_thread = bind("OpenThread", HANDLE, DWORD, BOOL, DWORD)
resume_thread = bind("ResumeThread", DWORD, HANDLE)
console_event = bind("GenerateConsoleCtrlEvent", BOOL, DWORD, DWORD)


def check(value):
    if not value:
        raise ctypes.WinError(ctypes.get_last_error())
    return value


def resume(pid):
    snapshot = snapshot_threads(0x00000004, 0)
    if snapshot == INVALID_HANDLE:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        entry = ThreadEntry()
        entry.size = ctypes.sizeof(entry)
        exists = first_thread(snapshot, ctypes.byref(entry))
        for _ in range(131072):
            if not exists:
                break
            if entry.process == pid:
                thread = check(open_thread(0x0002, False, entry.thread))
                try:
                    if resume_thread(thread) == 0xFFFFFFFF:
                        raise ctypes.WinError(ctypes.get_last_error())
                    return
                finally:
                    close(thread)
            entry.size = ctypes.sizeof(entry)
            exists = next_thread(snapshot, ctypes.byref(entry))
        raise RuntimeError("could not locate suspended child thread")
    finally:
        close(snapshot)


class JobProcess(subprocess.Popen):
    def __init__(self, args, **options):
        self.job = check(create_job(None, None))
        child_created = False
        try:
            limits = Limits()
            limits.basic.flags = 0x00002000  # KILL_ON_JOB_CLOSE
            check(set_information(self.job, 9, ctypes.byref(limits), ctypes.sizeof(limits)))
            super().__init__(args, creationflags=subprocess.CREATE_NEW_PROCESS_GROUP | 0x4, **options)
            child_created = True
            check(assign(self.job, self._handle))
            resume(self.pid)
        except BaseException:
            self.close_job()
            if child_created:
                self.kill()
                self.wait(timeout=10)
            raise

    def close_job(self):
        if self.job:
            close(self.job)
            self.job = None

    def group_alive(self):
        info = Accounting()
        check(query_information(self.job, 1, ctypes.byref(info), ctypes.sizeof(info), None))
        return info.active != 0

    def graceful_stop(self):
        check(console_event(1, self.pid))  # CTRL_BREAK_EVENT

    def force_stop(self):
        check(terminate(self.job, 1))

    def __exit__(self, *exc):
        self.close_job()
        return super().__exit__(*exc)
