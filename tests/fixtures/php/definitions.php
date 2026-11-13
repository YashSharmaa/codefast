<?php
declare(strict_types=1);

namespace App\Models;

use App\Contracts\HasName;

/** Definitions: kinds, parents, types, arity and flags. */
#[Entity('users')]
abstract class User extends Model implements HasName
{
    use SoftDeletes;

    public const TABLE = 'users';
    private const SECRET = 'x';

    public ?Profile $profile = null;
    protected static int $count = 0;
    private array $tags = [], $flags;
    var $legacy;

    /** @var Repo */
    protected $repo;

    /** @var callable */
    public $onSave;

    public \Closure $formatter;

    public function __construct(private readonly Mailer $mailer, public int $level = 1, string $plain = '')
    {
        parent::__construct();
    }

    abstract protected function build(int $a, ?string $b = null, string ...$rest): static;

    /** @return Profile */
    public function profile()
    {
        return $this->profile;
    }

    final public static function make(): self
    {
        return new static(new Mailer());
    }

    private function secret(): string|int
    {
        return self::SECRET;
    }

    public function rows(): iterable
    {
        yield 1;
    }

    protected function lazy(): ?\App\Support\Lazy
    {
        return null;
    }
}

interface Shape extends HasName, \Countable
{
    const SIDES = 0;

    public function area(): float;
}

trait Greets
{
    private function hello(): string
    {
        return 'hi';
    }

    abstract public function name(): string;
}

enum Suit: string implements Shape
{
    case Hearts = 'H';
    case Spades = 'S';

    const Wild = self::Spades;

    public function color(): string
    {
        return 'red';
    }
}

final readonly class Point
{
    public function __construct(public float $x, public float $y) {}
}

function helper(int $a, $b = 2, ...$rest): ?User
{
    function nested(): void {}

    return null;
}

if (!function_exists('App\Models\maybe')) {
    function maybe() {}
}

const VERSION = '1.0', BUILD = 7;
define('LEGACY_FLAG', true);
define('App\Models\NS_FLAG', 1);
