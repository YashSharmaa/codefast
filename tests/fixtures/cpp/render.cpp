// Definitions and uses: out-of-line members, calls, receivers, locals, lambdas.
#include "geo/shapes.hpp"
#include <algorithm>
using namespace geo::shapes;
using std::unique_ptr;
namespace fs = std::filesystem;

int geo::shapes::instances = 0;
int Shape::live = 0;
static int draw_calls;
static bool by_area(const Shape *a, const Shape *b);
namespace {
struct Session { Canvas *canvas; int depth; void reset(); };
int budget = 8;
void spend(int n) { budget -= n; }
} // namespace
void Session::reset() { depth = 0; }

Shape::~Shape() { --live; }

bool Shape::validate() const {
  if (id_ == 0 || !cache_)
    return false;
  cache_->flush();
  outline_.clear();
  stats_.draws++;
  on_change_ = nullptr;
  return check(id_) && this->filled() && Shape::live > 0;
}

void Shape::draw(Canvas &canvas, int layer) const {
  Point origin;
  Point corner{1, 2};
  Point *ptr = &origin;
  const Session &session = current();
  auto fresh = new Canvas(layer);
  auto owned = std::make_unique<Session>();
  auto shared = std::make_shared<geo::shapes::Point>(3, 4);
  unique_ptr<Shape> held;
  std::vector<Point> points;
  canvas.begin(layer);
  origin.x = corner.y;
  ptr->y = kMaxSides;
  session.canvas->line(origin, corner);
  fresh->clear();
  owned->reset();
  owned.release();
  shared->x++;
  held->draw(canvas);
  points.push_back(origin);
  points[0].x = 1;
  ++draw_calls;
  spend(1);
  geo::shapes::length(outline_, true);
  fs::remove(path());
  ::abort();
  auto *poly = static_cast<Poly<int> *>(this);
  poly->at(0);
  if (auto *other = dynamic_cast<const Shape *>(ptr2))
    other->area();
  Canvas temp(layer, Fill::Solid);
  temp.end();
  Canvas(layer).end();
  auto extent = canvas.bounds();
  extent.grow(layer, /*clamp=*/true);
  if (isa<Poly<int>>(ptr))
    cast<Shape>(ptr)->area();
  Ref<Shape> made = Shape::make("poly", 3);
  Fill mode = Fill::Solid;
  delete fresh;
  // receiver type paths: call results, static results, wrappers, local aliases
  bounds().grow(layer);
  Shape::make("p", 1)->area();
  geo::shapes::Shape::make("q", 2)->area();
  geo::shapes::current().reset();
  util::Timer(layer).stop();
  util::clock().now();
  canvas.layer<int>().clear();
  this->outline_.size();
  auto &first = outline().front();
  first.norm();
  std::optional<Point> maybe;
  maybe->norm();
  maybe.reset();
  using Pts = std::vector<Point>;
  typedef Canvas *CanvasPtr;
  Pts pts;
  pts.push_back(origin);
  CanvasPtr cp = fresh;
  cp->clear();
  Canvas probe(bounds());
  Canvas probe2(make_layer(layer), origin);
  Canvas probe3(cast<Canvas>(ptr));
  mode = geo::shapes::Fill::None;
  canvas.each(visit_point, &origin);
}

template <typename T, int N, typename... Rest>
double Poly<T, N, Rest...>::area() const {
  T total = T();
  for (const T &side : sides)
    total += side;
  for (auto &[key, value] : lookup_)
    use(key, value);
  for (int i = 0, n = count(); i < n; ++i)
    total += at(i);
  return total * tau<double>;
}

double geo::shapes::length(const Path &path, bool closed) {
  auto dist = [&](const Point &a, const Point &b) -> double { return hypot(a.x - b.x, a.y - b.y); };
  double sum = 0;
  for (auto it = path.begin(); it != path.end(); ++it)
    sum += dist(*it, path.front());
  std::sort(path.begin(), path.end(), by_area);
  apply(dist);
  try {
    risky(sum);
  } catch (const std::exception &err) {
    report(err.what());
  }
  if (int extra = bonus(); extra > 0)
    sum += extra;
  struct Local { int n; int twice() const { return n * 2; } };
  Local scratch{3};
  return closed ? sum + scratch.twice() : sum;
}

static auto on_exit = [](int code) { cleanup(code); };
static void (*hook)(Shape *) = notify;
Shape &geo::shapes::operator<<(Shape &s, const Point &p) { s.outline_.push_back(p); return s; }
Poly<int>::operator bool() const { return sides[0]; }
int (*pick(int which))(int) { return which ? twice : nullptr; }
Shape::Shape(Id id, Fill fill) : Tracked(id, fill), id_(id), on_change_(notify) { Tracked::touch(); }
inline Shape::operator bool() const { return id_ != 0; }
