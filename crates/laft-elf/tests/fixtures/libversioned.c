/* A shared library that exports every kind of symbol the ABI digest covers. */

int add_v1(int a, int b) { return a + b; }
int add_v2(int a, int b, int c) { return a + b + c; }
__asm__(".symver add_v1, add@VERS_1");
__asm__(".symver add_v2, add@@VERS_2");

int counter = 1;
__thread long tls_slot;

__attribute__((weak)) int weak_hook(void) { return 0; }
__attribute__((visibility("protected"))) int protected_fn(void) { return 2; }
__attribute__((visibility("hidden"))) int hidden_fn(void) { return 3; }
int unversioned_fn(void) { return hidden_fn(); }

static int indirect_impl(void) { return 4; }
static int (*indirect_resolver(void))(void) { return indirect_impl; }
int indirect(void) __attribute__((ifunc("indirect_resolver")));

__asm__(".globl marker\n.pushsection .data\nmarker:\n.byte 0\n.popsection");
