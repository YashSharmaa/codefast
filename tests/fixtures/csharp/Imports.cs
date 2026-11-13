global using System.Linq;
using System;
using System.IO;
using static System.Math;
using static Acme.Util.Strings;
using Json = Newtonsoft.Json.JsonConvert;
using Map = System.Collections.Generic.Dictionary<string, Acme.Model.Item>;
using Acme.Model;

namespace Acme.App;

class Loader
{
    Map cache = new();
    System.Text.StringBuilder buffer;
    Acme.Model.Item first;
    Acme.Model.Catalog.Entry entry;

    void Load(string path)
    {
        var text = File.ReadAllText(path);
        var exists = System.IO.File.Exists(path);
        var item = Json.DeserializeObject<Item>(text);
        var abs = Abs(-1);
        var trimmed = Clean(text);
        global::System.Console.WriteLine(text);
        Acme.Model.Item.Create();
        Catalog.Entry.Parse(text);
    }
}
