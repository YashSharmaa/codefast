namespace Acme.Pre
{
    public class Reader
    {
        public int Mode(int kind)
        {
            if (kind == 0)
            {
                return Zero();
            }
#if HAVE_EXTRA
            else if (kind == 1)
            {
                return One();
            }
#endif
            else
            {
                return Other();
            }
        }

        #region helpers
        int Zero() => 0;
        int One() => 1;
        int Other() => 2;
        #endregion

        public bool HasInfo() => true;
    }
}
