<?php
declare(strict_types=1);
namespace ThrowsProbe\documented;
/** @throws \DomainException */
function documented(): void { throw new \DomainException(); }
