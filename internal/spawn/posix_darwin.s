// Trampolines to the functions of posix_spawn in libSystem; see
// posix_darwin.go.

#include "textflag.h"

TEXT libc_posix_spawn_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn(SB)
GLOBL	·libc_posix_spawn_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_trampoline_addr(SB)/8, $libc_posix_spawn_trampoline<>(SB)

TEXT libc_posix_spawn_file_actions_init_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn_file_actions_init(SB)
GLOBL	·libc_posix_spawn_file_actions_init_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_file_actions_init_trampoline_addr(SB)/8, $libc_posix_spawn_file_actions_init_trampoline<>(SB)

TEXT libc_posix_spawn_file_actions_destroy_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn_file_actions_destroy(SB)
GLOBL	·libc_posix_spawn_file_actions_destroy_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_file_actions_destroy_trampoline_addr(SB)/8, $libc_posix_spawn_file_actions_destroy_trampoline<>(SB)

TEXT libc_posix_spawn_file_actions_adddup2_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn_file_actions_adddup2(SB)
GLOBL	·libc_posix_spawn_file_actions_adddup2_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_file_actions_adddup2_trampoline_addr(SB)/8, $libc_posix_spawn_file_actions_adddup2_trampoline<>(SB)

TEXT libc_posix_spawn_file_actions_addclose_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn_file_actions_addclose(SB)
GLOBL	·libc_posix_spawn_file_actions_addclose_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_file_actions_addclose_trampoline_addr(SB)/8, $libc_posix_spawn_file_actions_addclose_trampoline<>(SB)

TEXT libc_posix_spawn_file_actions_addinherit_np_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn_file_actions_addinherit_np(SB)
GLOBL	·libc_posix_spawn_file_actions_addinherit_np_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_file_actions_addinherit_np_trampoline_addr(SB)/8, $libc_posix_spawn_file_actions_addinherit_np_trampoline<>(SB)

TEXT libc_posix_spawn_file_actions_addchdir_np_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawn_file_actions_addchdir_np(SB)
GLOBL	·libc_posix_spawn_file_actions_addchdir_np_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawn_file_actions_addchdir_np_trampoline_addr(SB)/8, $libc_posix_spawn_file_actions_addchdir_np_trampoline<>(SB)

TEXT libc_posix_spawnattr_init_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawnattr_init(SB)
GLOBL	·libc_posix_spawnattr_init_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawnattr_init_trampoline_addr(SB)/8, $libc_posix_spawnattr_init_trampoline<>(SB)

TEXT libc_posix_spawnattr_destroy_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawnattr_destroy(SB)
GLOBL	·libc_posix_spawnattr_destroy_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawnattr_destroy_trampoline_addr(SB)/8, $libc_posix_spawnattr_destroy_trampoline<>(SB)

TEXT libc_posix_spawnattr_setflags_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawnattr_setflags(SB)
GLOBL	·libc_posix_spawnattr_setflags_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawnattr_setflags_trampoline_addr(SB)/8, $libc_posix_spawnattr_setflags_trampoline<>(SB)

TEXT libc_posix_spawnattr_setpgroup_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_posix_spawnattr_setpgroup(SB)
GLOBL	·libc_posix_spawnattr_setpgroup_trampoline_addr(SB), RODATA, $8
DATA	·libc_posix_spawnattr_setpgroup_trampoline_addr(SB)/8, $libc_posix_spawnattr_setpgroup_trampoline<>(SB)
