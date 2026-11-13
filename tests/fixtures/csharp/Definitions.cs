using System;
using System.Threading.Tasks;

namespace Acme.Shapes
{
    /// A shape with an area.
    public interface IShape
    {
        double Area();
        string Name { get; }
        event EventHandler Changed;
    }

    public abstract class Shape : IShape, IComparable<Shape>
    {
        public const int Sides = 0;
        private static readonly string Prefix = "shape:";
        protected int id;
        internal Func<int, int> Scale = x => x * 2;

        protected Shape(int id) { this.id = id; }
        ~Shape() { }

        public abstract double Area();
        public virtual string Name => Prefix + id;
        public int Id { get; private set; }
        public int this[int index] { get { return index; } set { id = value; } }
        public event EventHandler Changed;
        public event EventHandler Moved { add { Changed += value; } remove { Changed -= value; } }

        public int CompareTo(Shape other) => Area().CompareTo(other.Area());
        public static Shape operator +(Shape a, Shape b) => a;
        public static explicit operator double(Shape s) => s.Area();

        public async Task<int> CountAsync(int start, int step = 1, params int[] rest)
        {
            int Add(int a, int b) => a + b;
            await Task.Yield();
            return Add(start, step);
        }

        [DllImport("native")]
        static extern int NativeCount();

        void Hidden() { }
    }

    public sealed class Circle : Shape
    {
        public Circle(double radius) : base(1) { Radius = radius; }
        public double Radius { get; init; }
        public override double Area() => Math.PI * Radius * Radius;
        double IComparable<Circle>.Weight() => 0;
    }

    public struct Point
    {
        public int X, Y;
        public Point(int x, int y) { X = x; Y = y; }
    }

    public record Size(int Width, int Height);
    public record struct Pair(string Key, object Value);

    public enum Color { Red, Green = 2, Blue }

    public delegate void Notify(string message);

    public static class ShapeExtensions
    {
        public static double Doubled(this Shape shape) => shape.Area() * 2;
        public static T Largest<T>(this T[] shapes) where T : Shape => shapes[0];
    }

    public partial class Canvas<T> where T : IShape
    {
        private T current;
        T Pick() => current;
    }

    namespace Inner
    {
        class Nested
        {
            class Deeper { }
        }
    }
}
