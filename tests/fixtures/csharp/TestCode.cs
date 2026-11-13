using Xunit;
using NUnit.Framework;

namespace Acme.Tests;

public class ShapeTests
{
    [Fact]
    public void AreaIsPositive() { }

    [Theory]
    [InlineData(1)]
    public void ScalesWith(int factor) { }

    [Test, Category("slow")]
    public void Slow() { }

    [TestMethod]
    public void MsTest() { }

    public void Helper() { }
}
