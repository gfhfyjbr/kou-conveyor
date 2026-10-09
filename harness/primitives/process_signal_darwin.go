package primitives

import (
	"errors"
	"fmt"
	"syscall"

	"golang.org/x/sys/unix"
)

// Values from Darwin's sys/proc.h, not exported by x/sys/unix.
const (
	darwinProcessStateZombie = 5          // SZOMB
	darwinProcessFlagExiting = 0x00002000 // P_WEXIT
)

func normalizeProcessGroupSignalError(processGroupID int, signalErr error) error {
	if !errors.Is(signalErr, syscall.EPERM) {
		return signalErr
	}

	// Darwin can return EPERM for a group containing only exiting processes or
	// unreaped zombies. P_WEXIT is visible before the process reaches SZOMB.
	// Check every member: a dead leader does not imply its descendants are dead,
	// and a successful direct-PID signal cannot rule out a group permission error.
	members, err := unix.SysctlKinfoProcSlice("kern.proc.pgrp", processGroupID)
	if err != nil {
		return errors.Join(signalErr, fmt.Errorf("inspect process group %d: %w", processGroupID, err))
	}
	if processGroupHasLiveMembers(members) {
		return signalErr
	}
	return nil
}

func processGroupHasLiveMembers(members []unix.KinfoProc) bool {
	for _, member := range members {
		if member.Proc.P_stat != darwinProcessStateZombie && member.Proc.P_flag&darwinProcessFlagExiting == 0 {
			return true
		}
	}
	return false
}
