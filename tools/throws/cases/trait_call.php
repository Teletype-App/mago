<?php
declare(strict_types=1);
namespace ThrowsProbe\trait_call;

trait ThrowingTrait
{
    public function perform(): void { throw new \DomainException(); }
}

final class Worker { use ThrowingTrait; }
function caller(Worker $worker): void { $worker->perform(); }
