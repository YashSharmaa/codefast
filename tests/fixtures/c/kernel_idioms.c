#include <linux/fs.h>
#include "internal.h"
#define FOO_MAX 16
#define foo_wrap(x, y) do_foo(x, (y) + FOO_MAX)
static DEFINE_MUTEX(foo_lock);
DEFINE_PER_CPU(struct foo, foo_pcpu);
static LIST_HEAD(foo_list);
struct foo {
	int flags;
	struct bar *b;
	const struct file_operations *f_op;
	int (*handler)(struct foo *f, int x);
	spinlock_t lock;
};
typedef struct { int a; } anon_t;
enum color { RED, GREEN = 2, BLUE };
static int counter;
extern int ext_var;
int do_foo(struct foo *f, int n);
static inline int helper(struct foo *f) { return f->flags & 1; }
static int __init foo_init(void)
{
	struct foo *f = kmalloc(sizeof(*f), GFP_KERNEL);
	int ret;
	if (!f)
		return -ENOMEM;
	f->flags = FOO_MAX;
	f->b->count++;
	counter += 1;
	ret = f->f_op->read(f, 0);
	f->handler = helper;
	return f->handler(f, ret) + foo_wrap(f, 2);
}
module_init(foo_init);
EXPORT_SYMBOL_GPL(do_foo);
MODULE_LICENSE("GPL");
static const struct foo_ops my_ops = {
	.open = foo_open,
	.nested = { .cb = foo_cb },
};
