using System;
using System.Collections.Generic;
using Acme.Shapes;

namespace Acme.Render
{
    [Serializable]
    [Renderer("2d", Priority = 3)]
    public class Painter : Base, IPainter
    {
        private readonly Shape _shape;
        private Circle circle = new(2.0);
        private List<Shape> shapes = new List<Shape>();
        public event Notify Done;
        public Action<string> Log { get; set; }

        public Painter(Shape shape) : base(shape.Id) { _shape = shape; }

        public void Paint(Canvas<IShape> canvas)
        {
            var c = new Circle(1.0) { Radius = 2.0 };
            c.Area();
            _shape.Area();
            this._shape.Area();
            circle.Area();
            base.Paint(canvas);
            this.Reset();
            Reset();
            Helper.Assist(c);
            var area = c.Doubled();
            Shape s = new Circle(3.0);
            var d = (Circle)s;
            var e = s as Circle;
            if (s is Circle round && round.Radius > 1) { round.Area(); }
            var t = typeof(Painter);
            var n = nameof(Paint);
            Point p = new(1, 2);
            p.X = 3;
            var y = p.Y;
            Done += OnDone;
            Done -= OnDone;
            c.Changed += this.OnChanged;
            Log = Write;
            shapes.ForEach(Draw);
            shapes.Sort(Compare);
            Done?.Invoke("ok");
            Log("x");
            _shape?.Area();
            var r = canvas.Pick().Area();
            int count = shapes.Count;
            Color k = Color.Red;
            var size = new Size(1, 2) with { Width = 3 };
            Func<Shape, double> f = x => x.Area();
            var list = new Dictionary<string, List<Circle>>();
        }

        void OnDone(string message) { }
        void OnChanged(object sender, EventArgs args) { }
        void Write(string text) { }
        void Draw(Shape shape) { }
        static int Compare(Shape a, Shape b) => 0;
        void Reset() { }
    }
}
