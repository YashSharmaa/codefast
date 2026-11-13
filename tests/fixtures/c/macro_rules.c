// Symbol-defining macros and entry-point registrations (kernel style).
#include <linux/module.h>

SYSCALL_DEFINE3(read, unsigned int, fd, char __user *, buf, size_t, count)
{
	return ksys_read(fd, buf, count);
}

COMPAT_SYSCALL_DEFINE2(getitimer, int, which, struct old_itimerval32 __user *, value)
{
	return do_compat(which, value);
}

BPF_CALL_2(bpf_map_lookup_elem, struct bpf_map *, map, void *, key)
{
	return (unsigned long) map->ops->map_lookup_elem(map, key);
}

static ssize_t speed_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "%d\n", 1);
}
static DEVICE_ATTR_RO(speed);
DEVICE_ATTR_RW(duplex);

static DEFINE_MUTEX(foo_mutex);
DEFINE_PER_CPU(struct rq, runqueues);
DEFINE_STATIC_KEY_FALSE(sched_numa_balancing);
DEFINE_FREE(kfree, void *, if (_T) kfree(_T))
DEFINE_GUARD(mutex, struct mutex *, mutex_lock(_T), mutex_unlock(_T))
DEFINE_GUARD_COND(mutex, _try, mutex_trylock(_T))
DEFINE_SHOW_ATTRIBUTE(foo_stats);
TRACE_EVENT(sched_switch,
	TP_PROTO(bool preempt),
	TP_ARGS(preempt)
);
module_param(debug_level, int, 0644);

static int __init foo_init(void)
{
	return 0;
}
static void __exit foo_exit(void) {}
static int __init foo_setup(char *s) { return 1; }

module_init(foo_init);
module_exit(foo_exit);
__setup("foo=", foo_setup);
subsys_initcall(foo_init);
module_platform_driver(foo_driver);
EXPORT_SYMBOL_GPL(runqueues);
