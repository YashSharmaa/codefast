// Declarations: namespaces, classes, members, templates, aliases, enums.
#ifndef GEO_SHAPES_HPP
#define GEO_SHAPES_HPP
#include <memory>
#include <vector>
#include "geo/point.hpp"

namespace geo::shapes {

class Canvas;
struct Point { int x = 0, y = 0; };
enum Color { Red, Green = 2 };
enum class Fill : unsigned char { None, Solid };
union Scalar { int i; float f; };
typedef unsigned long Id;
using Path = std::vector<Point>;
template <typename T> using Ref = std::shared_ptr<T>;
template <typename T> concept Drawable = requires(T t) { t.draw(); };
template <typename T> constexpr T tau = T(6.28);
extern int instances;
constexpr int kMaxSides = 64;
enum { kDefaultLayer = 1 };

class Shape {
public:
  Shape() = default;
  explicit Shape(Id id, Fill fill = Fill::None) : id_(id), fill_(fill) {}
  Shape(const Shape &) = delete;
  virtual ~Shape();
  virtual double area() const = 0;
  virtual void draw(Canvas &canvas, int layer = 0) const;
  Id id() const { return id_; }
  bool filled() const { return fill_ != Fill::None && validate(); }
  static Ref<Shape> make(const char *kind, ...);
  bool operator==(const Shape &other) const { return id_ == other.id_; }
  explicit operator bool() const { return id_ != 0; }
  template <typename V> void accept(V &&visitor) const { visitor.visit(*this); }
  friend void swap(Shape &a, Shape &b) { exchange(a.id_, b.id_); }
  friend class Canvas;
  using Children = std::vector<std::unique_ptr<Shape>>;
  Children kids_;
  [[nodiscard]] friend bool same(const Shape &a, const Shape &b) { return a.id_ == b.id_; }
  static const int kVersion = 3;
  static int live;

protected:
  bool validate() const;
  Id id_ = 0;
  Fill fill_;
  mutable Path outline_;
  std::unique_ptr<Canvas> cache_;
  void (*on_change_)(Shape *) = nullptr;
  struct Stats { int draws; } stats_;
  enum { kInline = 4 };
  union { int tag; float weight; };
};

template <typename T, int N = 2, typename... Rest>
class Poly final : public Shape, private Ref<T> {
public:
  T sides[N];
  T &at(int i) { return sides[i]; }
  double area() const override;
  template <typename U> Poly<U, N> cast() const;
};

template <> struct Poly<void> { int none; };

template <typename T> T clamp(T v, T lo, T hi);
double length(const Path &path, bool closed = false);
inline int twice(int v) { return v * 2; }
Shape &operator<<(Shape &s, const Point &p);

} // namespace geo::shapes

namespace { int in_header; }
#endif
