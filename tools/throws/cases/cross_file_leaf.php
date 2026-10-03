<?php
declare(strict_types=1);
namespace ThrowsProbe\cross_file;

function leaf(): void { throw new \DomainException(); }
