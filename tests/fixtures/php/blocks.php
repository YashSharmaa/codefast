<?php
namespace Lib\One {
    class Alpha
    {
        public function run(): Beta
        {
            return new Beta();
        }
    }

    function one(): void {}
}

namespace Lib\Two {
    use Lib\One\Alpha;

    class Beta extends Alpha
    {
        public function go(): void
        {
            $this->run()->go();
            one();
        }
    }
}

namespace {
    function global_one(): void {}

    $a = new Lib\One\Alpha();
    $a->run();
}
