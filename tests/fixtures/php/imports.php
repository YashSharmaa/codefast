<?php
namespace App\Http;

use App\Models\User;
use App\Models\{Post, Comment as Note, function slugify, const MAX_LEN};
use App\Support;
use function App\Support\format_name;
use const App\Support\LIMIT;
use Exception;
use \Illuminate\Support\Str as S;

require_once __DIR__ . '/bootstrap.php';
require 'vendor/autoload.php';
include_once dirname(__FILE__) . '/lib/legacy.php';
include $path;

/** Imports and the resolution of qualified names. */
function controller(): void
{
    new User();
    new Post();
    new Note();
    slugify('a');
    format_name('b');
    echo LIMIT, MAX_LEN;
    S::snake('x');
    Support\Arr::get([], 'k');
    Support\helper();
    Models\Local::run();
    namespace\local();
    \global_fn();
    \App\Support\fmt();
    $e = new Exception('x');
    $f = new \Exception('y');
    throw new NotFoundException();
}
