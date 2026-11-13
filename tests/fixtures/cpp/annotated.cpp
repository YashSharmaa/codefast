// Preprocessor, annotation macros, gtest and the C idioms that also occur in C++.
#include <gtest/gtest.h>
#include "ops.h"
#define MAX_OPS 16
#define CHECKED(x, y) do_check(x, (y) + MAX_OPS)
#define LOG_AT(level) ::logging::Stream(level, __FILE__).out()

class API_EXPORT Engine : public Base<Engine>, private detail::Tracker {
public:
  API_EXPORT explicit Engine(int threads);
  API_EXPORT ~Engine();
  [[nodiscard]] API_EXPORT std::string name() const NO_DISCARD_REASON;
  bool ready() const ATTR_PURE { return state_ == Ready; }
  API_EXPORT static Engine *create(const Config &config);
  int jobs_ GUARDED_BY(mu_);
  void lock() ACQUIRE(mu_);
  PREFERRED_TYPE(bool)
  unsigned running_ : 1;

  DECLARE_GETTER(Jobs, jobs_)
  DEFINE_TYPE_ID(Engine)
  int queue_[4] GUARDED_BY(mu_) = {};

private:
  enum State { Idle, Ready };
  State state_;
};

struct API_EXPORT Config {
  API_EXPORT Config(int n) : threads(n) {}
  using Handle NODEBUG_ATTR = Engine *;
  int threads;
};
class API_EXPORT Hidden;
API_EXPORT llvm::Expected<Engine> open(const char *path NONNULL_ARG);
API_EXPORT ALWAYS_INLINE_ATTR Engine *current();
static INLINE_HINT int fast_path(int v) { return v; }
void WINAPI_CALL entry(int code) { fast_path(code); }
engine_inline bool quick(int v) { return v > 0; }
void shutdown() NOEXCEPT_ATTR;

struct ops {
  int (*open)(struct ops *self, int flags);
  void (*close)(struct ops *self);
  struct { int (*cb)(void); } nested;
};
static int my_open(struct ops *self, int flags) { return self->nested.cb() + CHECKED(flags, 1); }
static const struct ops table = {
  .open = my_open,
  .close = my_close,
  .nested = { .cb = my_cb },
};
static DEFINE_MUTEX(table_lock);
DEFINE_REGISTRY(engines);

int run_all(struct ops *o, int n) {
  o->close = my_close;
  o->open(o, n);
  (*o->close)(o);
  table.open(o, 0);
  return n;
}

Engine::Engine(int threads) : Base<Engine>(threads), detail::Tracker{}, jobs_(threads), state_(Idle) {
  Config local = {.threads = threads};
  configure(local);
}

TEST(EngineTest, StartsIdle) {
  Engine engine(2);
  EXPECT_FALSE(engine.ready());
}

TEST_F(EngineFixture, RunsJobs) {
  prepare();
  ASSERT_EQ(run_all(&ops_, 2), 2);
}

TEST_P(EngineParam, Scales) { EXPECT_GT(GetParam(), 0); }
TYPED_TEST(EngineTyped, Builds) { TypeParam value; use(value); }
MATCHER_P(HasJobs, count, "") { return arg.jobs() == count; }
BENCH_CASE("startup") { Engine::create(Config{}); }
STATISTIC(NumEngines, "Number of engines created");
DEFINE_ENTRY(int, entry_point, (int argc, char **argv)) { return argc + run_all(nullptr, argc); }
DEFINE_ENTRY(status_t, checked_entry, (int argc)) { return argc; }
namespace wrapped {
DEFINE_ENTRY(status_t, inner_entry, (const char *name, int flags)) { return flags + name[0]; }
}
#if defined(HOT)
HOT_ATTR USED_ATTR
#endif
int hot_path(int v) { return ++NumEngines + v; }
