package primitives

import (
	"errors"
	"os"
	"os/exec"
	"syscall"
	"testing"
	"time"

	"golang.org/x/sys/unix"
)

func TestProcessTerminationWithUnreapedZombie(t *testing.T) {
	for _, test := range []struct {
		name string
		run  func(*os.Process) error
	}{
		{name: "SIGTERM", run: func(process *os.Process) error {
			return signalProcessInvocation(process, syscall.SIGTERM)
		}},
		{name: "SIGKILL", run: func(process *os.Process) error {
			return signalProcessInvocation(process, syscall.SIGKILL)
		}},
		{name: "terminate", run: func(process *os.Process) error {
			return terminateProcess(process, processParentPipes{}, 0)
		}},
	} {
		t.Run(test.name, func(t *testing.T) {
			command := startDarwinProcessGroupMember(t, 0)
			if err := command.Process.Kill(); err != nil {
				t.Fatal(err)
			}
			waitForDarwinZombie(t, command.Process.Pid)
			if err := test.run(command.Process); err != nil {
				t.Fatalf("terminate unreaped zombie: %v", err)
			}
			if err := command.Wait(); err == nil {
				t.Fatal("process exited without a signal")
			}
			if command.ProcessState == nil || command.ProcessState.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
				t.Fatalf("process state = %v, want SIGKILL", command.ProcessState)
			}
		})
	}
}

func TestSignalProcessInvocationWithZombieLeaderAndLiveGroupMember(t *testing.T) {
	leader := startDarwinProcessGroupMember(t, 0)
	member := startDarwinProcessGroupMember(t, leader.Process.Pid)
	if err := leader.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	waitForDarwinZombie(t, leader.Process.Pid)
	if err := normalizeProcessGroupSignalError(leader.Process.Pid, syscall.EPERM); !errors.Is(err, syscall.EPERM) {
		t.Fatalf("permission error with live group member = %v, want EPERM", err)
	}
	if err := signalProcessInvocation(leader.Process, syscall.SIGKILL); err != nil {
		t.Fatalf("signal group with zombie leader: %v", err)
	}
	waitForDarwinZombie(t, member.Process.Pid)
	if err := signalProcessInvocation(leader.Process, syscall.SIGKILL); err != nil {
		t.Fatalf("signal group with two zombies: %v", err)
	}
	if err := member.Wait(); err == nil {
		t.Fatal("group member exited without a signal")
	}
	if member.ProcessState == nil || member.ProcessState.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
		t.Fatalf("group member state = %v, want SIGKILL", member.ProcessState)
	}
}

func startDarwinProcessGroupMember(t *testing.T, processGroupID int) *exec.Cmd {
	t.Helper()
	command := exec.Command("/bin/sleep", "30")
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true, Pgid: processGroupID}
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if command.ProcessState == nil {
			_ = command.Process.Kill()
			_ = command.Wait()
		}
	})
	return command
}

func waitForDarwinZombie(t *testing.T, pid int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	delay := time.Millisecond
	for {
		info, err := unix.SysctlKinfoProc("kern.proc.pid", pid)
		if err != nil {
			t.Fatalf("inspect process %d: %v", pid, err)
		}
		if info.Proc.P_stat == darwinProcessStateZombie {
			return
		}
		remaining := time.Until(deadline)
		if remaining <= 0 {
			t.Fatalf("process %d state = %d, want zombie", pid, info.Proc.P_stat)
		}
		time.Sleep(min(delay, remaining))
		delay = min(delay*2, 50*time.Millisecond)
	}
}

func TestNormalizeProcessGroupSignalError(t *testing.T) {
	command := startDarwinProcessGroupMember(t, 0)
	for _, want := range []error{nil, syscall.EPERM, syscall.ESRCH, syscall.EINVAL} {
		if err := normalizeProcessGroupSignalError(command.Process.Pid, want); !errors.Is(err, want) {
			t.Fatalf("live group: normalize %v = %v", want, err)
		}
	}
	if err := command.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	waitForDarwinZombie(t, command.Process.Pid)
	for _, want := range []error{syscall.ESRCH, syscall.EINVAL} {
		if err := normalizeProcessGroupSignalError(command.Process.Pid, want); !errors.Is(err, want) {
			t.Fatalf("zombie group: normalize %v = %v", want, err)
		}
	}
	if err := normalizeProcessGroupSignalError(command.Process.Pid, syscall.EPERM); err != nil {
		t.Fatalf("zombie group: normalize EPERM = %v", err)
	}
	if err := command.Wait(); err == nil {
		t.Fatal("process exited without a signal")
	}
	if err := normalizeProcessGroupSignalError(command.Process.Pid, syscall.EPERM); err != nil {
		t.Fatalf("reaped group: normalize EPERM = %v", err)
	}
}

func TestProcessGroupHasLiveMembers(t *testing.T) {
	const (
		runningState = 2      // SRUN
		stoppedState = 4      // SSTOP
		execFlag     = 0x4000 // P_EXEC
	)
	live := unix.KinfoProc{Proc: unix.ExternProc{P_stat: runningState, P_flag: execFlag}}
	stopped := unix.KinfoProc{Proc: unix.ExternProc{P_stat: stoppedState}}
	zombie := unix.KinfoProc{Proc: unix.ExternProc{P_stat: darwinProcessStateZombie}}
	exiting := unix.KinfoProc{Proc: unix.ExternProc{P_stat: runningState, P_flag: execFlag | darwinProcessFlagExiting}}
	for _, test := range []struct {
		name    string
		members []unix.KinfoProc
		want    bool
	}{
		{name: "empty"},
		{name: "zombie", members: []unix.KinfoProc{zombie}},
		{name: "exiting before zombie", members: []unix.KinfoProc{exiting}},
		{name: "all terminating", members: []unix.KinfoProc{zombie, exiting}},
		{name: "live", members: []unix.KinfoProc{live}, want: true},
		{name: "stopped", members: []unix.KinfoProc{stopped}, want: true},
		{name: "zombie leader with live member", members: []unix.KinfoProc{zombie, live}, want: true},
		{name: "exiting leader with live member", members: []unix.KinfoProc{exiting, live}, want: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := processGroupHasLiveMembers(test.members); got != test.want {
				t.Fatalf("has live members = %t, want %t", got, test.want)
			}
		})
	}
}
