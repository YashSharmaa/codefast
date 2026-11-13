<?php
namespace App\Services;

use App\Models\User;
use App\Models\Profile as P;
use App\Repos\UserRepo;

/** References: call forms, receivers, members, types and callables. */
class UserService extends BaseService
{
    private UserRepo $repo;
    private ?Cache $cache = null;

    public function __construct(UserRepo $repo, private Mailer $mailer)
    {
        parent::__construct($repo);
        $this->repo = $repo;
        $this->handler = [$this, 'onSaved'];
        $this->formatter = format_user(...);
    }

    public function calls(User $user, $untyped, P $profile): ?User
    {
        helper($user, 2);
        \strlen('x');
        $this->validate($user);
        $user->save();
        $user?->touch();
        $untyped->save();
        $this->repo->find(1)->delete();
        $this->mailer->send($user, ...$this->extra);
        self::log('a');
        static::log('b');
        parent::boot();
        User::query()->where('a', 1)->first();
        \App\Models\User::find(2);
        $profile->avatar();
        $copy = $user;
        $copy->save();
        $made = new User();
        $made->save();
        $found = User::find(3);
        $found->save();
        $result = make_user();
        $result->save();
        $chain = $this->repo->find(4);
        $chain->delete();
        $later = null;
        $later = new P();
        $later->avatar();
        $mixed = new User();
        $mixed = new P();
        $mixed->save();
        (new User())->save();
        $user::table();
        return $user;
    }

    public function members(User $user): void
    {
        $user->name = 'x';
        $n = $user->name;
        $user->count++;
        $user->tags[] = 'y';
        $user->profile->avatar = 'z';
        $this->cache->items['k'] = 1;
        User::$instances = [];
        $c = User::TABLE . static::PREFIX . parent::$shared;
        $s = Suit::Hearts->color();
        $k = User::class;
        $u = [User::class, 'create'];
    }

    public function types(Profile|null $p, (A&B)|null $ab, int $n): void
    {
        if ($p instanceof Admin) {
            $p->grant();
        }
        if ($n instanceof self) {
        }
        try {
            risky();
        } catch (NotFound | \RuntimeException $e) {
            $e->report();
        } catch (Failure $f) {
            $f->retry();
        }
        $closure = function (User $u) use ($p): void {
            $u->save();
            $p->avatar();
        };
        $arrow = fn(User $x) => $x->save();
        array_map([$this, 'transform'], []);
        usort($list, [static::class, 'compare']);
        register($this->transform(...), User::create(...), format_user(...));
        foreach ($this->repo->all() as $key => $item) {
            $item->save();
        }
        /** @var User $doc */
        $doc = $this->locate();
        $doc->save();
        /** @var P */
        $typed = make();
        $typed->avatar();
    }

    #[Route('/x', methods: ['GET'])]
    #[\Override]
    public function attributed(): void
    {
        global $app;
        static $cache = null;
        echo LIMIT, \App\LIMIT, PHP_EOL, __DIR__;
        $title = "Hello {$this->user->name()}!";
    }
}
