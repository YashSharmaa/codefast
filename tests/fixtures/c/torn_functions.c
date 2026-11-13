/* A file the grammar cannot parse as a whole: the braces of the two branches below do not
 * balance, so every function after them arrives in pieces. */
struct rq { unsigned long nr; };

#ifdef CONFIG_A
int pick(int x) {
	if (x) {
#else
int pick(long x) {
	if (x > 1) {
#endif
		return 1;
	}
	return 0;
}

unsigned long count_all(void)
{
	int i;
	unsigned long sum = 0;

	for_each_cpu(i)
		sum += cpu_rq(i)->nr;

	return sum;
}

/* Whether anything waits on this CPU. */
bool cpu_waits(int cpu)
{
	return read_it(&cpu_rq(cpu)->nr) != 0;
}

static int __init late_setup(char *s)
{
	return count_all() > 0;
}
