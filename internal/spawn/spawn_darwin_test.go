package spawn

import (
	"fmt"
	"os"
	"runtime"
	"runtime/debug"
	"syscall"
	"testing"
	"unsafe"
)

func init() {
	// ids prints the IDs of the process, its group and its session.
	helpers["ids"] = func() {
		session, err := syscall.Getsid(0)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		fmt.Println(os.Getpid(), syscall.Getpgrp(), session)
	}

	// footprint grows the heap by 192 MB, starts three commands while it
	// is that big, gives it back, and prints the footprint before, during
	// and after. KOU_SPAWN_START says how the commands start: "spawn",
	// "fork", or "none" for no commands.
	helpers["footprint"] = func() {
		start := os.Getenv("KOU_SPAWN_START")
		forceFork = start == "fork"
		runtime.GC()
		debug.FreeOSMemory()
		before := footprint()
		burst := make([][]byte, 192)
		for i := range burst {
			burst[i] = make([]byte, 1<<20)
			for j := 0; j < len(burst[i]); j += 4096 {
				burst[i][j] = 1
			}
		}
		during := footprint()
		for range 3 {
			if start == "none" {
				break
			}
			if err := Command("/usr/bin/true").Run(); err != nil {
				fmt.Fprintln(os.Stderr, err)
				os.Exit(1)
			}
		}
		runtime.KeepAlive(burst)
		burst = nil
		runtime.GC()
		runtime.GC()
		debug.FreeOSMemory()
		fmt.Println(before, during, footprint())
	}
}

// footprint is the process's phys_footprint, from proc_pid_rusage: the
// memory Activity Monitor shows for it.
func footprint() int64 {
	const (
		sysProcInfo           = 336
		procInfoCallPidRusage = 9
		rusageInfoV4          = 4
	)
	var info [64]uint64 // a struct rusage_info_v4, and room
	_, _, errno := syscall.Syscall6(sysProcInfo, procInfoCallPidRusage, uintptr(os.Getpid()), rusageInfoV4,
		0, uintptr(unsafe.Pointer(&info)), 0)
	if errno != 0 {
		fmt.Fprintln(os.Stderr, "proc_pid_rusage:", errno)
		os.Exit(1)
	}
	return int64(info[9]) // ri_phys_footprint
}

func TestTheHeapGoesBackAfterCommandsStart(t *testing.T) {
	type footprints struct{ before, during, after int64 }
	const MB = 1 << 20
	measured := map[string]footprints{}
	for _, start := range []string{"none", "spawn", "fork"} {
		cmd := helperCommand(t, "footprint")
		cmd.Env = append(cmd.Env, "KOU_SPAWN_START="+start)
		out, err := cmd.Output()
		if err != nil {
			t.Fatal(err)
		}
		var f footprints
		if _, err := fmt.Sscan(string(out), &f.before, &f.during, &f.after); err != nil {
			t.Fatalf("%q: %v", out, err)
		}
		measured[start] = f
		t.Logf("footprint with %s, MB: %d before, %d with the heap, %d once it is given back", start, f.before/MB, f.during/MB, f.after/MB)
	}
	// Without commands, the heap given back must leave the footprint;
	// it does not with the race detector, or under Rosetta.
	none := measured["none"]
	if grown := none.during - none.before; grown < 100*MB || none.after-none.before > grown/2 {
		t.Skipf("without commands, the footprint went %d, %d, %d MB: the heap does not come and go here", none.before/MB, none.during/MB, none.after/MB)
	}
	if spawn := measured["spawn"]; spawn.after-none.after > (spawn.during-spawn.before)/4 {
		t.Errorf("with commands started, the footprint stayed at %d MB of %d; without, it fell to %d", spawn.after/MB, spawn.during/MB, none.after/MB)
	}
}

func TestProcessGroupAndSession(t *testing.T) {
	ourSession, err := syscall.Getsid(0)
	if err != nil {
		t.Fatal(err)
	}
	ids := func(t *testing.T, attr *syscall.SysProcAttr) (pid, group, session int) {
		t.Helper()
		cmd := helperCommand(t, "ids")
		cmd.SysProcAttr = attr
		out, err := cmd.Output()
		if err != nil {
			t.Fatal(err)
		}
		if _, err := fmt.Sscan(string(out), &pid, &group, &session); err != nil || pid != cmd.Process.Pid {
			t.Fatalf("the command %d printed %q: %v", cmd.Process.Pid, out, err)
		}
		return pid, group, session
	}
	eachMode(t, func(t *testing.T) {
		if _, group, session := ids(t, nil); group != syscall.Getpgrp() || session != ourSession {
			t.Errorf("by default: group %d and session %d, want ours: %d and %d", group, session, syscall.Getpgrp(), ourSession)
		}
		if pid, group, session := ids(t, &syscall.SysProcAttr{Setpgid: true}); group != pid || session != ourSession {
			t.Errorf("Setpgid: process %d in group %d and session %d, want a group of its own in ours", pid, group, session)
		}
		if pid, group, session := ids(t, &syscall.SysProcAttr{Setsid: true}); group != pid || session != pid {
			t.Errorf("Setsid: process %d in group %d and session %d, want a session of its own", pid, group, session)
		}

		leader := Command("/bin/sleep", "10")
		leader.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
		if err := leader.Start(); err != nil {
			t.Fatal(err)
		}
		defer func() {
			leader.Process.Kill()
			leader.Wait()
		}()
		if _, group, _ := ids(t, &syscall.SysProcAttr{Setpgid: true, Pgid: leader.Process.Pid}); group != leader.Process.Pid {
			t.Errorf("Pgid: group %d, want %d", group, leader.Process.Pid)
		}
	})
}

func TestWhatPosixSpawnCannotDoStartsWithOsExec(t *testing.T) {
	for _, test := range []struct {
		attr *syscall.SysProcAttr
		want bool
	}{
		{nil, true},
		{&syscall.SysProcAttr{}, true},
		{&syscall.SysProcAttr{Setpgid: true}, true},
		{&syscall.SysProcAttr{Setpgid: true, Pgid: 7}, true},
		{&syscall.SysProcAttr{Setsid: true}, true},
		{&syscall.SysProcAttr{Setsid: true, Setpgid: true}, false},
		{&syscall.SysProcAttr{Setctty: true}, false},
		{&syscall.SysProcAttr{Noctty: true}, false},
		{&syscall.SysProcAttr{Foreground: true}, false},
		{&syscall.SysProcAttr{Ptrace: true}, false},
		{&syscall.SysProcAttr{Chroot: "/"}, false},
		{&syscall.SysProcAttr{Credential: &syscall.Credential{}}, false},
	} {
		if got := spawnable(test.attr); got != test.want {
			t.Errorf("spawnable(%+v) = %v, want %v", test.attr, got, test.want)
		}
	}

	cmd := Command("/usr/bin/true")
	if err := cmd.Run(); err != nil || !cmd.spawned {
		t.Errorf("Run() = %v; posix_spawn made the process: %v, want true", err, cmd.spawned)
	}
	cmd = Command("/usr/bin/true")
	cmd.SysProcAttr = &syscall.SysProcAttr{Credential: &syscall.Credential{
		Uid: uint32(os.Getuid()), Gid: uint32(os.Getgid()), NoSetGroups: true,
	}}
	if err := cmd.Run(); err != nil || cmd.spawned {
		t.Errorf("with Credential: Run() = %v; posix_spawn made the process: %v, want false", err, cmd.spawned)
	}
}
