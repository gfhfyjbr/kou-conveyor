//go:build darwin

package spawn

import (
	"runtime"
	"syscall"
	"unsafe"
)

// Exec replaces this process, preserving descriptors not marked close-on-exec.
// Darwin's SETEXEC performs the replacement without syscall.Exec's unbounded
// wait for Go preemption signals, which native libraries can block or consume.
func Exec(path string, argv, env []string) error {
	pathPtr, err := syscall.BytePtrFromString(path)
	if err != nil {
		return err
	}
	argvPtrs, err := syscall.SlicePtrFromStrings(argv)
	if err != nil {
		return err
	}
	envPtrs, err := syscall.SlicePtrFromStrings(env)
	if err != nil {
		return err
	}
	var attributes uintptr
	if err := call1(libc_posix_spawnattr_init_trampoline_addr, &attributes); err != nil {
		return err
	}
	defer call1(libc_posix_spawnattr_destroy_trampoline_addr, &attributes)
	const posixSpawnSetexec = 0x0040 // POSIX_SPAWN_SETEXEC, <sys/spawn.h>
	if err := call3(libc_posix_spawnattr_setflags_trampoline_addr, &attributes, posixSpawnSetexec, 0); err != nil {
		return err
	}
	var pid int32
	result, _, _ := syscall_syscall6(libc_posix_spawn_trampoline_addr,
		uintptr(unsafe.Pointer(&pid)),
		uintptr(unsafe.Pointer(pathPtr)),
		0, // No file actions: keep the same close-on-exec semantics as execve.
		uintptr(unsafe.Pointer(&attributes)),
		uintptr(unsafe.Pointer(&argvPtrs[0])),
		uintptr(unsafe.Pointer(&envPtrs[0])))
	runtime.KeepAlive(pathPtr)
	runtime.KeepAlive(argvPtrs)
	runtime.KeepAlive(envPtrs)
	return errnoOf(result)
}
