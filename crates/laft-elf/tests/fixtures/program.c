/* A dynamic executable that imports from libversioned.so.1 without libc. */

extern int add(int, int, int);
extern int counter;
extern int optional_hook(void) __attribute__((weak));

void _start(void) {
  long status = add(1, 2, 3) + counter + (optional_hook ? optional_hook() : 0);
  __asm__ volatile("syscall" : : "a"(60), "D"(status) : "rcx", "r11", "memory");
  __builtin_unreachable();
}
