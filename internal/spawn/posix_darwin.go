//go:build darwin

package spawn

import (
	"runtime"
	"syscall"
	"unsafe"
)

// The flags of posix_spawnattr_setflags, from <spawn.h>.
const (
	posixSpawnSetpgroup      = 0x0002
	posixSpawnSetsid         = 0x0400
	posixSpawnCloexecDefault = 0x4000
)

// spawnable reports whether posix_spawn can start a process with attr:
// none, or a process group or a session of its own.
func spawnable(attr *syscall.SysProcAttr) bool {
	if forceFork {
		return false
	}
	if attr == nil {
		return true
	}
	rest := *attr
	if rest.Setsid && rest.Setpgid {
		return false // fork's child does them in an order of its own
	}
	rest.Setpgid, rest.Pgid, rest.Setsid = false, 0, false
	return rest == syscall.SysProcAttr{}
}

// posixSpawn starts path with posix_spawn and returns its process ID. The
// child gets fds[i] as its descriptor i and no other; it starts in dir
// when dir is set.
func posixSpawn(path string, argv, env []string, dir string, fds []int, attr *syscall.SysProcAttr) (int, error) {
	pathPtr, err := syscall.BytePtrFromString(path)
	if err != nil {
		return 0, err
	}
	argvPtrs, err := syscall.SlicePtrFromStrings(argv)
	if err != nil {
		return 0, err
	}
	envPtrs, err := syscall.SlicePtrFromStrings(env)
	if err != nil {
		return 0, err
	}
	var dirPtr *byte
	if dir != "" {
		if dirPtr, err = syscall.BytePtrFromString(dir); err != nil {
			return 0, err
		}
	}
	flags := posixSpawnCloexecDefault
	pgid := 0
	if attr != nil {
		if attr.Setpgid {
			flags |= posixSpawnSetpgroup
			pgid = attr.Pgid
		}
		if attr.Setsid {
			flags |= posixSpawnSetsid
		}
	}

	// posix_spawn_file_actions_t and posix_spawnattr_t: pointers to what
	// libc allocates.
	var actions, attributes uintptr
	if err := call1(libc_posix_spawn_file_actions_init_trampoline_addr, &actions); err != nil {
		return 0, err
	}
	defer call1(libc_posix_spawn_file_actions_destroy_trampoline_addr, &actions)
	if err := call1(libc_posix_spawnattr_init_trampoline_addr, &attributes); err != nil {
		return 0, err
	}
	defer call1(libc_posix_spawnattr_destroy_trampoline_addr, &attributes)

	if dirPtr != nil {
		if err := addChdir(&actions, dirPtr); err != nil {
			return 0, err
		}
	}
	for _, action := range fileActions(fds) {
		var err error
		switch action.op {
		case dupTo:
			err = call3(libc_posix_spawn_file_actions_adddup2_trampoline_addr, &actions, action.fd, action.to)
		case keep:
			err = call3(libc_posix_spawn_file_actions_addinherit_np_trampoline_addr, &actions, action.fd, 0)
		case closeFD:
			err = call3(libc_posix_spawn_file_actions_addclose_trampoline_addr, &actions, action.fd, 0)
		}
		if err != nil {
			return 0, err
		}
	}
	if flags&posixSpawnSetpgroup != 0 {
		if err := call3(libc_posix_spawnattr_setpgroup_trampoline_addr, &attributes, pgid, 0); err != nil {
			return 0, err
		}
	}
	if err := call3(libc_posix_spawnattr_setflags_trampoline_addr, &attributes, flags, 0); err != nil {
		return 0, err
	}

	var pid int32
	r, _, _ := syscall_syscall6(libc_posix_spawn_trampoline_addr,
		uintptr(unsafe.Pointer(&pid)),
		uintptr(unsafe.Pointer(pathPtr)),
		uintptr(unsafe.Pointer(&actions)),
		uintptr(unsafe.Pointer(&attributes)),
		uintptr(unsafe.Pointer(&argvPtrs[0])),
		uintptr(unsafe.Pointer(&envPtrs[0])))
	runtime.KeepAlive(pathPtr)
	runtime.KeepAlive(argvPtrs)
	runtime.KeepAlive(envPtrs)
	if err := errnoOf(r); err != nil {
		return 0, err
	}
	return int(pid), nil
}

// call1 calls one of the functions of posix_spawn that take only the
// object they set up.
func call1(fn uintptr, object *uintptr) error {
	r, _, _ := syscall_syscall6(fn, uintptr(unsafe.Pointer(object)), 0, 0, 0, 0, 0)
	return errnoOf(r)
}

// call3 calls one that also takes up to two numbers.
func call3(fn uintptr, object *uintptr, a, b int) error {
	r, _, _ := syscall_syscall6(fn, uintptr(unsafe.Pointer(object)), uintptr(a), uintptr(b), 0, 0, 0)
	return errnoOf(r)
}

func addChdir(actions *uintptr, dir *byte) error {
	r, _, _ := syscall_syscall6(libc_posix_spawn_file_actions_addchdir_np_trampoline_addr,
		uintptr(unsafe.Pointer(actions)), uintptr(unsafe.Pointer(dir)), 0, 0, 0, 0)
	return errnoOf(r)
}

// errnoOf is the error the functions of posix_spawn return: an errno, or
// 0. They return an int; only its 32 bits count.
func errnoOf(r uintptr) error {
	if errno := syscall.Errno(uint32(r)); errno != 0 {
		return errno
	}
	return nil
}

// The functions are libSystem's, called as package syscall calls them on
// darwin: through trampolines, without cgo.

//go:linkname syscall_syscall6 syscall.syscall6
func syscall_syscall6(fn, a1, a2, a3, a4, a5, a6 uintptr) (r1, r2 uintptr, err syscall.Errno)

var libc_posix_spawn_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn posix_spawn "/usr/lib/libSystem.B.dylib"

var libc_posix_spawn_file_actions_init_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn_file_actions_init posix_spawn_file_actions_init "/usr/lib/libSystem.B.dylib"

var libc_posix_spawn_file_actions_destroy_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn_file_actions_destroy posix_spawn_file_actions_destroy "/usr/lib/libSystem.B.dylib"

var libc_posix_spawn_file_actions_adddup2_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn_file_actions_adddup2 posix_spawn_file_actions_adddup2 "/usr/lib/libSystem.B.dylib"

var libc_posix_spawn_file_actions_addclose_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn_file_actions_addclose posix_spawn_file_actions_addclose "/usr/lib/libSystem.B.dylib"

var libc_posix_spawn_file_actions_addinherit_np_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn_file_actions_addinherit_np posix_spawn_file_actions_addinherit_np "/usr/lib/libSystem.B.dylib"

var libc_posix_spawn_file_actions_addchdir_np_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawn_file_actions_addchdir_np posix_spawn_file_actions_addchdir_np "/usr/lib/libSystem.B.dylib"

var libc_posix_spawnattr_init_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawnattr_init posix_spawnattr_init "/usr/lib/libSystem.B.dylib"

var libc_posix_spawnattr_destroy_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawnattr_destroy posix_spawnattr_destroy "/usr/lib/libSystem.B.dylib"

var libc_posix_spawnattr_setflags_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawnattr_setflags posix_spawnattr_setflags "/usr/lib/libSystem.B.dylib"

var libc_posix_spawnattr_setpgroup_trampoline_addr uintptr

//go:cgo_import_dynamic libc_posix_spawnattr_setpgroup posix_spawnattr_setpgroup "/usr/lib/libSystem.B.dylib"
