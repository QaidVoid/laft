/* A static executable without libc. */

void _start(void) {
  __asm__ volatile("syscall" : : "a"(60), "D"(0) : "rcx", "r11", "memory");
  __builtin_unreachable();
}
