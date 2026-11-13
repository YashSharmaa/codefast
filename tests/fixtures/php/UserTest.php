<?php
namespace Tests\Unit;

use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\Test;

/** Test code: test methods by name, by docblock and by attribute. */
class UserTest extends TestCase
{
    protected function setUp(): void
    {
        parent::setUp();
    }

    public function testSaves(): void
    {
        $this->assertTrue(true);
    }

    /** @test */
    public function it_loads(): void {}

    #[Test]
    public function deletes(): void {}

    public function helper(): void {}
}

class Fixture
{
    public function testLooksLikeATest(): void {}
}
