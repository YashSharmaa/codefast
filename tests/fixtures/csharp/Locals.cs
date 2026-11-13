using System;
using System.Linq;

namespace Acme.Locals
{
    class Scopes
    {
        Repo repo;

        void Run(Repo r, int[] ids, (int A, string B) pair)
        {
            foreach (var id in ids) { r.Find(id).Save(); }
            foreach (Item item in r.All()) { item.Save(); }
            foreach (var (key, value) in r.Map()) { key.Use(); }
            var (first, second) = r.Split();
            first.Use();
            using (var tx = r.Begin()) { tx.Commit(); }
            using var scope = new Scope();
            scope.Close();
            try { r.Save(); }
            catch (RepoException ex) when (ex.Code > 0) { ex.Log(); }
            if (r.Get() is Item found) { found.Save(); }
            if (r.TryGet(1, out var got)) { got.Save(); }
            if (r.TryGet(2, out Item typed)) { typed.Save(); }
            var names = from it in r.All() let nm = it.Name where nm != null select nm.Trim();
            var byId = r.All().Select(x => x.Id).Where((v, i) => i > 0);
            var res = r.Get() switch { Item { Name: "a" } named => named.Save(), _ => 0 };
            Func<Item, int> f = delegate (Item z) { return z.Id; };
            int Local(int q) => q + repo.Count;
            Local(1);
            var copy = repo;
            copy.Save();
            value = 3;
        }
    }
}
