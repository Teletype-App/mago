<?php
declare(strict_types=1);
namespace ThrowsProbe\synchronous_callback;

/** @param callable(): void $callback */
function invoke(callable $callback): void { $callback(); }
function caller(): void
{
    invoke(static function (): void { throw new \DomainException(); });
}
