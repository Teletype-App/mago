<?php
declare(strict_types=1);
namespace ThrowsProbe\chain_documented_leaf;
/** @throws \DomainException */
function leaf(): void { throw new \DomainException(); }
function middle(): void { leaf(); }
function caller(): void { middle(); }
