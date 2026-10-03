<?php
declare(strict_types=1);
namespace ThrowsProbe\broad_docblock;
/** @throws \Throwable */
function caller(): void { throw new \DomainException(); }
