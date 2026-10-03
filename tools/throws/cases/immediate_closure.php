<?php
declare(strict_types=1);
namespace ThrowsProbe\immediate_closure;

function caller(): void
{
    (static function (): void { throw new \DomainException(); })();
}
